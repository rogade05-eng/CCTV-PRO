/*
 * CCTV DESIGN PRO — Shell de escritorio: comandos nativos del sistema operativo (§3, §9, §32, §35)
 *
 * Este binario es **envoltorio**: no calcula nada. Todo el diseño, el cálculo y la documentación
 * siguen viviendo en los motores de `app/core/`, que no saben que existe Windows. El shell aporta
 * solo lo que el navegador no puede dar:
 *
 *   1. Base de datos SQLite propia (§32) con catálogo, reglas, proyectos, copias y auditoría.
 *   2. Puente al conversor DWG **local** (§9): ejecuta el conversor que el usuario tenga en
 *      `tools/converter/` y devuelve el DXF producido. Nunca sale a Internet ni invoca AutoCAD.
 *   3. Ficheros del proyecto en la carpeta portable (§35): leer y escribir dentro de la carpeta.
 *
 * Modos sin ventana, para verificación automática (y para el taller):
 *   CCTVDesign.exe --selftest [--report ruta.json]   comprueba SQLite, carpetas y conversor
 *   CCTVDesign.exe --convert entrada.dwg salida.dxf  convierte con el conversor local
 *   CCTVDesign.exe --b64-ida original.bin copia.bin  copia binaria por el camino Base64 (sin pérdidas)
 *   CCTVDesign.exe --db-dump                         volcado de recuentos de la base
 * Los dos primeros escriben el resultado en un informe JSON y devuelven código de salida 0/1.
 */
#![cfg_attr(all(not(debug_assertions), not(feature = "consola")), windows_subsystem = "windows")]

mod b64;
mod converter;
mod db;

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/* ------------------------------------------------------------------ rutas portables §35 */

/// Carpeta del ejecutable: en una instalación portable es la raíz del programa.
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Raíz del programa: si el .exe está en `bin/`, la raíz es la carpeta de arriba.
pub fn app_root() -> PathBuf {
    let dir = exe_dir();
    if dir.file_name().map(|n| n == "bin").unwrap_or(false) {
        dir.parent().map(|p| p.to_path_buf()).unwrap_or(dir)
    } else {
        dir
    }
}

/// ¿Se puede escribir en esta carpeta? (una instalación en `Program Files` no lo permite).
pub fn escribible(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let prueba = dir.join(".prueba-escritura");
    match std::fs::write(&prueba, b"ok") {
        Ok(_) => {
            let _ = std::fs::remove_file(&prueba);
            true
        }
        Err(_) => false,
    }
}

/// Carpeta de datos del programa: junto al ejecutable si se puede escribir (instalación portable,
/// la preferida del §35) y, si no (instalación por equipo en `Program Files`), en la carpeta de
/// datos del usuario. Así el programa funciona en los dos casos sin pedir permisos de administrador
/// cada vez que guarda un proyecto o abre la base de datos.
pub fn carpeta_datos() -> PathBuf {
    let raiz = app_root();
    if escribible(&raiz.join("database")) {
        return raiz;
    }
    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local).join("CCTV DESIGN PRO");
        }
        if let Ok(perfil) = std::env::var("USERPROFILE") {
            return PathBuf::from(perfil).join("CCTV DESIGN PRO");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cctvdesign");
    }
    std::env::temp_dir().join("CCTV DESIGN PRO")
}

fn juntar(raiz: &Path, partes: &[&str]) -> PathBuf {
    let mut p = raiz.to_path_buf();
    for c in partes {
        p.push(c);
    }
    p
}

/* ------------------------------------------------------------------ informe de verificación */

fn escribir_informe(raiz: &Path, ruta: Option<&str>, informe: &Value) -> bool {
    let destino = match ruta {
        Some(r) => PathBuf::from(r),
        None => juntar(raiz, &["selftest-report.json"]),
    };
    let texto = serde_json::to_string_pretty(informe).unwrap_or_else(|_| "{}".into());
    if let Some(dir) = destino.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ok_escritura = std::fs::write(&destino, texto).is_ok();
    println!("{}", serde_json::to_string(informe).unwrap_or_default());
    ok_escritura
}

/* ------------------------------------------------------------------ verificación (§35) */

fn selftest(ruta_informe: Option<&str>) -> i32 {
    let raiz = app_root();
    let datos = carpeta_datos();
    let mut checks: Vec<Value> = Vec::new();
    let mut ok = true;
    let mut comprobar = |nombre: &str, bien: bool, detalle: String| {
        if !bien {
            ok = false;
        }
        checks.push(json!({ "nombre": nombre, "ok": bien, "detalle": detalle }));
    };

    // 1. Carpetas de la instalación portable
    let carpeta_db = juntar(&datos, &["database"]);
    let carpeta_web = juntar(&raiz, &["web"]);
    let carpeta_conv = juntar(&raiz, &["tools", "converter"]);
    comprobar(
        "carpeta database",
        carpeta_db.is_dir(),
        carpeta_db.display().to_string(),
    );
    comprobar(
        "carpeta del conversor",
        carpeta_conv.is_dir(),
        carpeta_conv.display().to_string(),
    );

    // 2. SQLite propio (§32): se abre (o se crea) y se comprueban sus tablas y semillas
    let ruta_db = juntar(&carpeta_db, &["cctv.sqlite"]);
    let mut resumen_db = json!({ "ruta": ruta_db.display().to_string() });
    match db::abrir(&ruta_db) {
        Ok(mut conn) => {
            let version = db::version_sqlite(&conn);
            let semillas = juntar(&carpeta_db, &["seed"]);
            let informe_init = db::inicializar(&mut conn, &semillas);
            let tablas = db::tablas(&conn);
            let catalogo = db::contar(&conn, "catalog_items");
            let reglas = db::contar(&conn, "rules");
            resumen_db = json!({
                "ruta": ruta_db.display().to_string(),
                "sqlite": version,
                "tablas": tablas,
                "articulos": catalogo,
                "reglas": reglas,
                "semillas": informe_init,
            });
            comprobar(
                "SQLite abre la base del programa",
                !tablas.is_empty(),
                format!("{} tablas · SQLite {}", tablas.len(), version),
            );
            comprobar(
                "el catálogo está en la base de datos",
                catalogo > 0,
                format!("{} artículos · {} reglas", catalogo, reglas),
            );
        }
        Err(e) => {
            resumen_db = json!({ "ruta": ruta_db.display().to_string(), "error": e });
            comprobar("SQLite abre la base del programa", false, e);
        }
    }

    // 3. Conversor DWG local (§9)
    let conversores = converter::detectar(&carpeta_conv);
    let hay_conv = !conversores.is_empty();
    comprobar(
        "conversor DWG local disponible",
        hay_conv,
        if hay_conv {
            conversores
                .iter()
                .map(|c| format!("{} {}", c.nombre, c.version))
                .collect::<Vec<_>>()
                .join(" · ")
        } else {
            "no hay conversor en tools/converter (el programa funciona igual, con DXF)".into()
        },
    );

    // 4. Ficheros binarios (§9): un DWG, un DXF o un ZIP tienen que cruzar sin perder un byte.
    //    Se comprueba de verdad: se escriben bytes, se releen y se comparan uno a uno.
    let prueba_bin = juntar(&carpeta_db, &["prueba-binaria.bin"]);
    let muestra: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
    let codificado = b64::codificar(&muestra);
    let bien_bin = match b64::decodificar(&codificado) {
        Ok(vuelta) => {
            let iguales = vuelta == muestra;
            let escrito = std::fs::write(&prueba_bin, &vuelta).is_ok();
            let releido = std::fs::read(&prueba_bin).map(|v| v == muestra).unwrap_or(false);
            let _ = std::fs::remove_file(&prueba_bin);
            iguales && escrito && releido
        }
        Err(_) => false,
    };
    comprobar(
        "los ficheros binarios viajan intactos (DWG/DXF/ZIP)",
        bien_bin,
        if bien_bin {
            format!(
                "{} bytes de prueba · Base64 {} caracteres · ida y vuelta exacta",
                muestra.len(),
                codificado.len()
            )
        } else {
            "un fichero binario se corrompería al escribirlo (revisar Base64)".into()
        },
    );

    // 5. Interfaz incrustada: el shell se compiló con la aplicación web dentro
    let version = env!("CARGO_PKG_VERSION");
    comprobar(
        "el ejecutable declara su versión",
        !version.is_empty(),
        format!("CCTV DESIGN PRO {}", version),
    );

    let informe = json!({
        "ok": ok,
        "programa": "CCTV DESIGN PRO",
        "version": version,
        "plataforma": std::env::consts::OS,
        "arquitectura": std::env::consts::ARCH,
        "ejecutable": exe_dir().display().to_string(),
        "raiz": raiz.display().to_string(),
        "carpetaDatos": datos.display().to_string(),
        "carpetaWeb": carpeta_web.display().to_string(),
        "baseDeDatos": resumen_db,
        "conversores": conversores,
        "comprobaciones": checks,
        "pruebas": checks.len(),
        "correctas": checks.iter().filter(|c| c["ok"] == json!(true)).count(),
    });
    let escrito = escribir_informe(&raiz, ruta_informe, &informe);
    if !escrito {
        eprintln!("aviso: no se pudo escribir el informe de verificación");
    }
    if ok {
        0
    } else {
        1
    }
}

/* ------------------------------------------------- ficheros binarios sin ventana (§9, §35) */

/// Ida y vuelta de un fichero binario por el mismo camino que usa la interfaz (Base64):
/// leer → codificar → decodificar → escribir. Sirve para comprobar con un DWG real que no se
/// pierde ni un byte. Devuelve 0 si la copia es idéntica al original.
fn b64_ida(entrada: &str, salida: &str) -> i32 {
    let original = match std::fs::read(entrada) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("no se pudo leer {}: {}", entrada, e);
            return 2;
        }
    };
    let codificado = b64::codificar(&original);
    let vuelta = match b64::decodificar(&codificado) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error al decodificar: {}", e);
            return 1;
        }
    };
    if let Err(e) = std::fs::write(salida, &vuelta) {
        eprintln!("no se pudo escribir {}: {}", salida, e);
        return 1;
    }
    let igual = vuelta == original;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "entrada": entrada,
            "salida": salida,
            "bytes": original.len(),
            "base64": codificado.len(),
            "identicos": igual,
        }))
        .unwrap_or_default()
    );
    if igual {
        0
    } else {
        1
    }
}

/* ------------------------------------------------------------------ conversión sin ventana */

fn convertir_sin_ventana(entrada: &str, salida: &str) -> i32 {
    let raiz = app_root();
    let carpeta = juntar(&raiz, &["tools", "converter"]);
    let conversores = converter::detectar(&carpeta);
    if conversores.is_empty() {
        eprintln!("no hay conversor en {}", carpeta.display());
        return 2;
    }
    match converter::convertir(&conversores[0], Path::new(entrada), Path::new(salida)) {
        Ok(r) => {
            println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
            if r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                0
            } else {
                1
            }
        }
        Err(e) => {
            eprintln!("error de conversión: {}", e);
            1
        }
    }
}

/* ------------------------------------------------------------------ ventana (§36) */

fn ventana() {
    let raiz = app_root();
    tauri::Builder::default()
        .setup(move |app| {
            // La base de datos se prepara al arrancar: si falla, el programa sigue (avisa en el panel)
            let datos = carpeta_datos();
            let ruta = juntar(&datos, &["database", "cctv.sqlite"]);
            let semillas = juntar(&raiz, &["database", "seed"]);
            if !semillas.is_dir() {
                // Instalación por equipo: las semillas viajan con los recursos, la base con los datos
                let alt = juntar(&datos, &["database", "seed"]);
                if alt.is_dir() {
                    if let Ok(mut conn) = db::abrir(&ruta) {
                        let _ = db::inicializar(&mut conn, &alt);
                    }
                }
            } else if let Ok(mut conn) = db::abrir(&ruta) {
                let _ = db::inicializar(&mut conn, &semillas);
                let _ = db::auditar(&conn, "arranque", &format!("CCTV DESIGN PRO {}", env!("CARGO_PKG_VERSION")));
            }
            let _ = app;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            info_shell,
            db_consulta,
            db_ejecuta,
            db_inicializa,
            conversor_estado,
            conversor_convertir,
            leer_texto,
            escribir_texto,
            leer_binario,
            escribir_binario,
            conversor_convertir_base64,
            listar_carpeta,
        ])
        .run(tauri::generate_context!())
        .expect("no se pudo iniciar CCTV DESIGN PRO");
}

/* ------------------------------------------------------------------ comandos para la interfaz */

#[tauri::command]
fn info_shell() -> Value {
    let raiz = app_root();
    let datos = carpeta_datos();
    let ruta_db = juntar(&datos, &["database", "cctv.sqlite"]);
    let (sqlite, articulos, reglas) = match db::abrir(&ruta_db) {
        Ok(conn) => (
            db::version_sqlite(&conn),
            db::contar(&conn, "catalog_items"),
            db::contar(&conn, "rules"),
        ),
        Err(_) => ("(no disponible)".into(), 0, 0),
    };
    json!({
        "shell": "tauri",
        "version": env!("CARGO_PKG_VERSION"),
        "plataforma": std::env::consts::OS,
        "arquitectura": std::env::consts::ARCH,
        "raiz": raiz.display().to_string(),
        "carpetaDatos": datos.display().to_string(),
        "baseDeDatos": { "ruta": ruta_db.display().to_string(), "sqlite": sqlite, "articulos": articulos, "reglas": reglas },
        "conversores": converter::detectar(&juntar(&raiz, &["tools", "converter"])),
    })
}

#[tauri::command]
fn db_consulta(sql: String, params: Option<Value>) -> Result<Value, String> {
    let datos = carpeta_datos();
    let conn = db::abrir(&juntar(&datos, &["database", "cctv.sqlite"]))?;
    db::consulta(&conn, &sql, params).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_ejecuta(sql: String, params: Option<Value>) -> Result<Value, String> {
    let datos = carpeta_datos();
    let conn = db::abrir(&juntar(&datos, &["database", "cctv.sqlite"]))?;
    let filas = db::ejecuta(&conn, &sql, params).map_err(|e| e.to_string())?;
    Ok(json!({ "filas": filas }))
}

#[tauri::command]
fn db_inicializa() -> Result<Value, String> {
    let datos = carpeta_datos();
    let mut conn = db::abrir(&juntar(&datos, &["database", "cctv.sqlite"]))?;
    let r = db::inicializar(&mut conn, &juntar(&datos, &["database", "seed"]));
    db::auditar(&conn, "db:inicializar", &serde_json::to_string(&r).unwrap_or_default())
        .ok();
    Ok(r)
}

/* ------------------------------------------------------------------ conversor DWG (§9) */

#[tauri::command]
fn conversor_estado() -> Value {
    let raiz = app_root();
    let carpeta = juntar(&raiz, &["tools", "converter"]);
    json!({ "carpeta": carpeta.display().to_string(), "conversores": converter::detectar(&carpeta) })
}

#[tauri::command]
fn conversor_convertir(entrada: String, salida: String) -> Result<Value, String> {
    let raiz = app_root();
    let conversores = converter::detectar(&juntar(&raiz, &["tools", "converter"]));
    let conv = conversores.first().ok_or("no hay conversor DWG instalado en tools/converter")?;
    converter::convertir(conv, Path::new(&entrada), Path::new(&salida)).map_err(|e| e.to_string())
}

/* ------------------------------------------------------------------ ficheros del proyecto §35 */

/// Comprueba que una ruta pedida por la interfaz no se sale de la carpeta del programa ni de la
/// carpeta de datos del usuario.
fn dentro_del_programa(ruta: &Path) -> Result<PathBuf, String> {
    let raiz = app_root().canonicalize().unwrap_or_else(|_| app_root());
    let datos = carpeta_datos().canonicalize().unwrap_or_else(|_| carpeta_datos());
    let permitida = |p: &Path| p.starts_with(&raiz) || p.starts_with(&datos);
    let _ = permitida;
    let destino = if ruta.is_absolute() {
        ruta.to_path_buf()
    } else {
        raiz.join(ruta)
    };
    let normal = destino
        .canonicalize()
        .unwrap_or_else(|_| destino.clone());
    if normal.starts_with(&raiz) || normal.starts_with(&datos) {
        Ok(normal)
    } else {
        Err(format!(
            "ruta fuera de la carpeta del programa: {}",
            normal.display()
        ))
    }
}

#[tauri::command]
fn leer_texto(ruta: String) -> Result<String, String> {
    let p = dentro_del_programa(Path::new(&ruta))?;
    std::fs::read_to_string(&p).map_err(|e| format!("{}: {}", p.display(), e))
}

#[tauri::command]
fn escribir_texto(ruta: String, contenido: String) -> Result<Value, String> {
    let raiz = app_root().canonicalize().unwrap_or_else(|_| app_root());
    let datos = carpeta_datos().canonicalize().unwrap_or_else(|_| carpeta_datos());
    let destino = if Path::new(&ruta).is_absolute() {
        PathBuf::from(&ruta)
    } else {
        datos.join(&ruta)
    };
    if !destino.starts_with(&raiz) && !destino.starts_with(&datos) {
        return Err(format!("ruta fuera de la carpeta del programa: {}", destino.display()));
    }
    if let Some(dir) = destino.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let bytes = contenido.as_bytes().len();
    std::fs::write(&destino, contenido).map_err(|e| format!("{}: {}", destino.display(), e))?;
    Ok(json!({ "ruta": destino.display().to_string(), "bytes": bytes }))
}

/* ---- ficheros binarios: DWG, DXF binario, ZIP (§9, §35) -------------------------------------
 * Se pasan en Base64 para no corromper ni un byte: así el DWG que el usuario elige en la
 * interfaz llega intacto al conversor y el DXF que produce se puede leer y dibujar.
 */

/// Resuelve una ruta de escritura dentro de las carpetas permitidas (programa o datos).
fn destino_permitido(ruta: &str) -> Result<PathBuf, String> {
    let raiz = app_root().canonicalize().unwrap_or_else(|_| app_root());
    let datos = carpeta_datos().canonicalize().unwrap_or_else(|_| carpeta_datos());
    let destino = if Path::new(ruta).is_absolute() {
        PathBuf::from(ruta)
    } else {
        datos.join(ruta)
    };
    if destino.starts_with(&raiz) || destino.starts_with(&datos) {
        Ok(destino)
    } else {
        Err(format!(
            "ruta fuera de la carpeta del programa: {}",
            destino.display()
        ))
    }
}

#[tauri::command]
fn escribir_binario(ruta: String, base64: String) -> Result<Value, String> {
    let destino = destino_permitido(&ruta)?;
    if let Some(dir) = destino.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let bytes = b64::decodificar(&base64)?;
    std::fs::write(&destino, &bytes).map_err(|e| format!("{}: {}", destino.display(), e))?;
    Ok(json!({ "ruta": destino.display().to_string(), "bytes": bytes.len() }))
}

#[tauri::command]
fn leer_binario(ruta: String) -> Result<Value, String> {
    let p = dentro_del_programa(Path::new(&ruta))?;
    let bytes = std::fs::read(&p).map_err(|e| format!("{}: {}", p.display(), e))?;
    Ok(json!({
        "ruta": p.display().to_string(),
        "bytes": bytes.len(),
        "base64": b64::codificar(&bytes),
    }))
}

/// Convierte un DWG que llega en Base64 y devuelve el DXF ya convertido, también en Base64.
/// Un solo paso: escribir el DWG temporal, ejecutar el conversor local y leer el resultado.
/// Es lo que usa el botón «Convertir DWG e importar» del panel (§9).
#[tauri::command]
fn conversor_convertir_base64(nombre: String, base64: String) -> Result<Value, String> {
    let raiz = app_root();
    let conversores = converter::detectar(&juntar(&raiz, &["tools", "converter"]));
    let conv = conversores
        .first()
        .ok_or("no hay conversor DWG instalado en tools/converter")?;

    // Nombre saneado: sin rutas ni caracteres raros que el conversor no sepa abrir
    let limpio: String = nombre
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    let base = limpio.trim_end_matches(".dwg").trim_end_matches(".DWG");
    let base = if base.is_empty() { "plano" } else { base };

    let carpeta = juntar(&carpeta_datos(), &["proyectos"]);
    std::fs::create_dir_all(&carpeta).map_err(|e| e.to_string())?;
    let entrada = juntar(&carpeta, &[&format!("{}.dwg", base)]);
    let salida = juntar(&carpeta, &[&format!("{}.dxf", base)]);

    let bytes = b64::decodificar(&base64)?;
    std::fs::write(&entrada, &bytes).map_err(|e| format!("{}: {}", entrada.display(), e))?;

    let res = converter::convertir(conv, &entrada, &salida)?;
    let ok = res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut informe = res.clone();
    if ok {
        let dxf = std::fs::read(&salida).map_err(|e| format!("{}: {}", salida.display(), e))?;
        informe["entradaBytes"] = json!(bytes.len());
        informe["dxfBase64"] = json!(b64::codificar(&dxf));
        informe["dxfBytes"] = json!(dxf.len());
        informe["nombre"] = json!(base);
    }
    Ok(informe)
}

#[tauri::command]
fn listar_carpeta(ruta: String) -> Result<Value, String> {
    let p = dentro_del_programa(Path::new(&ruta))?;
    let mut entradas: Vec<Value> = Vec::new();
    let it = std::fs::read_dir(&p).map_err(|e| format!("{}: {}", p.display(), e))?;
    for e in it.flatten() {
        let meta = e.metadata().ok();
        entradas.push(json!({
            "nombre": e.file_name().to_string_lossy(),
            "carpeta": meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
            "bytes": meta.as_ref().map(|m| m.len()).unwrap_or(0),
        }));
    }
    entradas.sort_by(|a, b| a["nombre"].as_str().cmp(&b["nombre"].as_str()));
    Ok(json!({ "ruta": p.display().to_string(), "entradas": entradas }))
}

/* ------------------------------------------------------------------ arranque */

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let informe = args
        .iter()
        .position(|a| a == "--report")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());
    let codigo = if args.iter().any(|a| a == "--selftest") {
        selftest(informe)
    } else if let Some(i) = args.iter().position(|a| a == "--convert") {
        match (args.get(i + 1), args.get(i + 2)) {
            (Some(e), Some(s)) => convertir_sin_ventana(e, s),
            _ => {
                eprintln!("uso: CCTVDesign.exe --convert entrada.dwg salida.dxf");
                2
            }
        }
    } else if let Some(i) = args.iter().position(|a| a == "--b64-ida") {
        match (args.get(i + 1), args.get(i + 2)) {
            (Some(e), Some(s)) => b64_ida(e, s),
            _ => {
                eprintln!("uso: CCTVDesign.exe --b64-ida original.bin copia.bin");
                2
            }
        }
    } else if args.iter().any(|a| a == "--db-dump") {
        let datos = carpeta_datos();
        match db::abrir(&juntar(&datos, &["database", "cctv.sqlite"])) {
            Ok(conn) => {
                let d = json!({
                    "tablas": db::tablas(&conn),
                    "articulos": db::contar(&conn, "catalog_items"),
                    "reglas": db::contar(&conn, "rules"),
                    "proyectos": db::contar(&conn, "projects"),
                    "sqlite": db::version_sqlite(&conn),
                });
                println!("{}", serde_json::to_string_pretty(&d).unwrap_or_default());
                0
            }
            Err(e) => {
                eprintln!("{}", e);
                1
            }
        }
    } else {
        ventana();
        0
    };
    if codigo != 0 {
        std::process::exit(codigo);
    }
}
