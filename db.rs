/*
 * CCTV DESIGN PRO — Base de datos propia (§32)
 *
 * El contrato exige que el programa tenga **su** base de datos, sin gestores externos ni
 * servicios: SQLite dentro de la carpeta portable. Aquí viven el catálogo de equipos, las reglas
 * de cálculo, los proyectos abiertos, las copias de seguridad y la auditoría de cambios.
 *
 * Las semillas (`database/seed/catalogo.json` y `database/seed/reglas.json`) las genera
 * `tools/package.mjs` a partir del catálogo y las reglas que ya usa el programa, de modo que la base
 * y el código nunca se separan: lo que el ingeniero ve en pantalla es lo que hay en la base.
 */
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, params_from_iter};
use serde_json::{json, Map, Value};
use std::path::Path;

pub const ESQUEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS meta (
  clave TEXT PRIMARY KEY,
  valor TEXT
);

CREATE TABLE IF NOT EXISTS catalog_items (
  id           TEXT PRIMARY KEY,
  categoria    TEXT NOT NULL,
  fabricante   TEXT,
  modelo       TEXT,
  nombre       TEXT,
  unidad       TEXT,
  precio       REAL DEFAULT 0,
  especif      TEXT,          -- JSON con las características técnicas (§24)
  actualizado  TEXT
);
CREATE INDEX IF NOT EXISTS ix_catalogo_categoria ON catalog_items (categoria);
CREATE INDEX IF NOT EXISTS ix_catalogo_modelo    ON catalog_items (modelo);

CREATE TABLE IF NOT EXISTS rules (
  clave     TEXT PRIMARY KEY,
  valor     REAL,
  unidad    TEXT,
  etiqueta  TEXT,
  grupo     TEXT,
  ayuda     TEXT
);
CREATE INDEX IF NOT EXISTS ix_reglas_grupo ON rules (grupo);

CREATE TABLE IF NOT EXISTS projects (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  nombre      TEXT NOT NULL,
  codigo      TEXT,
  cliente     TEXT,
  modificado  TEXT,
  ruta        TEXT,
  version     INTEGER DEFAULT 1,
  datos       TEXT              -- proyecto .cctv completo en JSON (§6)
);
CREATE UNIQUE INDEX IF NOT EXISTS ix_proyectos_nombre ON projects (nombre);

CREATE TABLE IF NOT EXISTS backups (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  proyecto   TEXT NOT NULL,
  fecha      TEXT NOT NULL,
  motivo     TEXT,
  datos      TEXT NOT NULL      -- copia completa con marca de tiempo (§37)
);

CREATE TABLE IF NOT EXISTS audit (
  id       INTEGER PRIMARY KEY AUTOINCREMENT,
  fecha    TEXT NOT NULL,
  accion   TEXT NOT NULL,
  detalle  TEXT
);

CREATE TABLE IF NOT EXISTS converters (
  nombre    TEXT PRIMARY KEY,
  ejecutable TEXT,
  version   TEXT,
  origen    TEXT,
  sha256    TEXT,
  detectado INTEGER DEFAULT 1
);
"#;

fn ahora() -> String {
    // Fecha ISO sin depender de cajas externas: segundos desde época → fecha legible aproximada
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dias = t / 86_400;
    let seg = t % 86_400;
    // Cálculo civil (Howard Hinnant) — sin bibliotecas de fechas
    let z = dias as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        seg / 3600,
        (seg % 3600) / 60,
        seg % 60
    )
}

/// Abre (y crea si hace falta) la base de datos del programa, con su esquema aplicado.
pub fn abrir(ruta: &Path) -> Result<Connection, String> {
    if let Some(dir) = ruta.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    }
    let conn = Connection::open(ruta).map_err(|e| format!("{}: {}", ruta.display(), e))?;
    conn.execute_batch(ESQUEMA).map_err(|e| e.to_string())?;
    Ok(conn)
}

pub fn version_sqlite(conn: &Connection) -> String {
    conn.query_row("SELECT sqlite_version()", [], |r| r.get::<_, String>(0))
        .unwrap_or_else(|_| "?".into())
}

pub fn tablas(conn: &Connection) -> Vec<String> {
    let mut st = match conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let filas = st.query_map([], |r| r.get::<_, String>(0));
    match filas {
        Ok(f) => f.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

pub fn contar(conn: &Connection, tabla: &str) -> i64 {
    // El nombre de tabla no se interpola desde la interfaz: solo llega desde este mismo archivo
    let sql = format!("SELECT COUNT(*) FROM {}", tabla);
    conn.query_row(&sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0)
}

pub fn auditar(conn: &Connection, accion: &str, detalle: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO audit (fecha, accion, detalle) VALUES (?1, ?2, ?3)",
        rusqlite::params![ahora(), accion, detalle],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Semillas: catálogo y reglas exportados por `tools/package.mjs` desde los motores del programa.
pub fn inicializar(conn: &mut Connection, carpeta_semillas: &Path) -> Value {
    let mut informe = json!({ "catalogo": 0, "reglas": 0, "semillas": carpeta_semillas.display().to_string() });

    let catalogo = carpeta_semillas.join("catalogo.json");
    if let Ok(texto) = std::fs::read_to_string(&catalogo) {
        if let Ok(v) = serde_json::from_str::<Value>(&texto) {
            let items = v.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
            let tx = conn.transaction().ok();
            if let Some(tx) = tx {
                let mut n = 0;
                for it in &items {
                    let specs = it.get("specs").cloned().unwrap_or(json!({})).to_string();
                    let r = tx.execute(
                        "INSERT INTO catalog_items (id, categoria, fabricante, modelo, nombre, unidad, precio, especif, actualizado)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                         ON CONFLICT(id) DO UPDATE SET
                           categoria=excluded.categoria, fabricante=excluded.fabricante, modelo=excluded.modelo,
                           nombre=excluded.nombre, unidad=excluded.unidad, precio=excluded.precio,
                           especif=excluded.especif, actualizado=excluded.actualizado",
                        rusqlite::params![
                            it.get("id").and_then(|x| x.as_str()).unwrap_or_default(),
                            it.get("category").and_then(|x| x.as_str()).unwrap_or("otros"),
                            it.get("manufacturer").and_then(|x| x.as_str()).unwrap_or(""),
                            it.get("model").and_then(|x| x.as_str()).unwrap_or(""),
                            it.get("name").and_then(|x| x.as_str()).unwrap_or(""),
                            it.get("unit").and_then(|x| x.as_str()).unwrap_or("ud"),
                            it.get("price").and_then(|x| x.as_f64()).unwrap_or(0.0),
                            specs,
                            ahora(),
                        ],
                    );
                    if r.is_ok() {
                        n += 1;
                    }
                }
                let version = v.get("version").cloned().unwrap_or(json!(1)).to_string();
                let moneda = v.get("currency").and_then(|c| c.as_str()).unwrap_or("EUR");
                let _ = tx.execute(
                    "INSERT INTO meta (clave, valor) VALUES ('catalogo_version', ?1)
                     ON CONFLICT(clave) DO UPDATE SET valor=excluded.valor",
                    rusqlite::params![version],
                );
                let _ = tx.execute(
                    "INSERT INTO meta (clave, valor) VALUES ('moneda', ?1)
                     ON CONFLICT(clave) DO UPDATE SET valor=excluded.valor",
                    rusqlite::params![moneda],
                );
                let _ = tx.commit();
                informe["catalogo"] = json!(n);
            }
        }
    }

    let reglas = carpeta_semillas.join("reglas.json");
    if let Ok(texto) = std::fs::read_to_string(&reglas) {
        if let Ok(v) = serde_json::from_str::<Value>(&texto) {
            let lista = v.get("rules").and_then(|r| r.as_array()).cloned().unwrap_or_default();
            let tx = conn.transaction().ok();
            if let Some(tx) = tx {
                let mut n = 0;
                for r in &lista {
                    let res = tx.execute(
                        "INSERT INTO rules (clave, valor, unidad, etiqueta, grupo, ayuda)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(clave) DO UPDATE SET
                           valor=excluded.valor, unidad=excluded.unidad, etiqueta=excluded.etiqueta,
                           grupo=excluded.grupo, ayuda=excluded.ayuda",
                        rusqlite::params![
                            r.get("key").and_then(|x| x.as_str()).unwrap_or_default(),
                            r.get("value").and_then(|x| x.as_f64()).unwrap_or(0.0),
                            r.get("unit").and_then(|x| x.as_str()).unwrap_or(""),
                            r.get("label").and_then(|x| x.as_str()).unwrap_or(""),
                            r.get("group").and_then(|x| x.as_str()).unwrap_or(""),
                            r.get("help").and_then(|x| x.as_str()).unwrap_or(""),
                        ],
                    );
                    if res.is_ok() {
                        n += 1;
                    }
                }
                let _ = tx.commit();
                informe["reglas"] = json!(n);
            }
        }
    }

    let _ = auditar(
        conn,
        "db:semillas",
        &format!(
            "catálogo {} · reglas {}",
            informe["catalogo"], informe["reglas"]
        ),
    );
    informe
}

fn a_sql(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(if *b { 1 } else { 0 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlValue::Integer(i)
            } else {
                SqlValue::Real(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => SqlValue::Text(s.clone()),
        otro => SqlValue::Text(otro.to_string()),
    }
}

fn params(v: Option<Value>) -> Vec<SqlValue> {
    match v {
        Some(Value::Array(a)) => a.iter().map(a_sql).collect(),
        Some(Value::Object(o)) => o.values().map(a_sql).collect(),
        Some(otro) => vec![a_sql(&otro)],
        None => Vec::new(),
    }
}

/// Consulta libre de solo lectura: devuelve las filas como objetos JSON.
pub fn consulta(conn: &Connection, sql: &str, p: Option<Value>) -> Result<Value, rusqlite::Error> {
    let mut st = conn.prepare(sql)?;
    let columnas: Vec<String> = st.column_names().iter().map(|c| c.to_string()).collect();
    let valores = params(p);
    let filas = st.query_map(params_from_iter(valores.iter()), |row| {
        let mut obj = Map::new();
        for (i, c) in columnas.iter().enumerate() {
            let v: Value = match row.get_ref(i)? {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(n) => json!(n),
                rusqlite::types::ValueRef::Real(f) => json!(f),
                rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                rusqlite::types::ValueRef::Blob(_) => json!("(binario)"),
            };
            obj.insert(c.clone(), v);
        }
        Ok(Value::Object(obj))
    })?;
    Ok(Value::Array(filas.flatten().collect()))
}

/// Escritura (INSERT/UPDATE/DELETE) con parámetros: devuelve el número de filas afectadas.
pub fn ejecuta(conn: &Connection, sql: &str, p: Option<Value>) -> Result<usize, rusqlite::Error> {
    let valores = params(p);
    conn.execute(sql, params_from_iter(valores.iter()))
}

/// Guarda un proyecto y su copia de seguridad previa (§37).
pub fn guardar_proyecto(conn: &Connection, nombre: &str, datos: &str, motivo: &str) -> Result<Value, String> {
    let anterior: Option<String> = conn
        .query_row("SELECT datos FROM projects WHERE nombre = ?1", rusqlite::params![nombre], |r| r.get(0))
        .ok();
    if let Some(previo) = anterior {
        conn.execute(
            "INSERT INTO backups (proyecto, fecha, motivo, datos) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![nombre, ahora(), motivo, previo],
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute(
        "INSERT INTO projects (nombre, modificado, version, datos) VALUES (?1, ?2, 1, ?3)
         ON CONFLICT(nombre) DO UPDATE SET modificado=excluded.modificado, datos=excluded.datos,
           version = projects.version + 1",
        rusqlite::params![nombre, ahora(), datos],
    )
    .map_err(|e| e.to_string())?;
    let version: i64 = conn
        .query_row("SELECT version FROM projects WHERE nombre = ?1", rusqlite::params![nombre], |r| r.get(0))
        .unwrap_or(1);
    Ok(json!({ "proyecto": nombre, "version": version, "guardado": ahora() }))
}

/// Catálogo completo desde la base (lo consume la interfaz al arrancar en el shell).
pub fn catalogo(conn: &Connection) -> Result<Value, String> {
    let filas = consulta(conn, "SELECT id, categoria, fabricante, modelo, nombre, unidad, precio, especif FROM catalog_items ORDER BY categoria, id", None)
        .map_err(|e| e.to_string())?;
    Ok(json!({ "items": filas }))
}

/// Reglas desde la base (clave → valor, como las espera el motor de cálculo).
pub fn reglas(conn: &Connection) -> Result<Value, String> {
    let filas = consulta(conn, "SELECT clave, valor FROM rules", None).map_err(|e| e.to_string())?;
    let mut mapa = Map::new();
    if let Some(a) = filas.as_array() {
        for f in a {
            if let (Some(k), Some(v)) = (f.get("clave").and_then(|x| x.as_str()), f.get("valor").and_then(|x| x.as_f64())) {
                mapa.insert(k.to_string(), json!(v));
            }
        }
    }
    Ok(json!({ "rules": mapa }))
}
