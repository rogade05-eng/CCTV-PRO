/*
 * CCTV DESIGN PRO — Puente al conversor DWG local (§9)
 *
 * El contrato **prohíbe** que el programa espere a AutoCAD o a un servicio de red. La estrategia
 * del §9 es: identificar la versión del DWG por su firma, convertir **en el equipo del usuario** con
 * el conversor que allí haya (LibreDWG `dwg2dxf`, ODA File Converter, o el CAD propio que exporta a
 * DXF) y leer el DXF resultante con el lector propio.
 *
 * Este módulo es el paso «3. Importar» del asistente: busca el conversor en `tools/converter/`,
 * lo ejecuta sin abrir ventana, comprueba que el DXF existe y devuelve la traza completa.
 * Si no hay conversor, no pasa nada: el programa sigue funcionando con DXF (§9).
 */
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversorInfo {
    pub nombre: String,
    pub ejecutable: String,
    pub version: String,
    pub origen: String,
    pub args_fijos: Vec<String>,
}

/// Manifiesto admitido en `tools/converter/converter.json` (lo escribe `tools/fetch-converter.mjs`).
#[derive(Debug, Deserialize)]
struct Manifiesto {
    nombre: String,
    ejecutable: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    origen: String,
    #[serde(default)]
    sha256: String,
    #[serde(default)]
    argumentos: Vec<String>,
}

#[cfg(windows)]
const CREAR_SIN_VENTANA: u32 = 0x0800_0000; // CREATE_NO_WINDOW

fn ejecutar(exe: &Path, args: &[String]) -> (String, String, Option<i32>) {
    let mut cmd = Command::new(exe);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREAR_SIN_VENTANA);
    }
    match cmd.output() {
        Ok(salida) => (
            String::from_utf8_lossy(&salida.stdout).to_string(),
            String::from_utf8_lossy(&salida.stderr).to_string(),
            salida.status.code(),
        ),
        Err(e) => (String::new(), e.to_string(), None),
    }
}

/// Conversores disponibles en la carpeta indicada, en orden de preferencia.
pub fn detectar(carpeta: &Path) -> Vec<ConversorInfo> {
    let mut encontrados: Vec<ConversorInfo> = Vec::new();

    if let Ok(texto) = std::fs::read_to_string(carpeta.join("converter.json")) {
        if let Ok(m) = serde_json::from_str::<Manifiesto>(&texto) {
            let exe = carpeta.join(&m.ejecutable);
            if exe.is_file() {
                let version = if m.version.is_empty() {
                    version_de(&exe)
                } else {
                    m.version.clone()
                };
                encontrados.push(ConversorInfo {
                    nombre: m.nombre,
                    ejecutable: exe.display().to_string(),
                    version,
                    origen: m.origen,
                    args_fijos: m.argumentos,
                });
            }
        }
    }

    // Detección directa por nombre: aunque no haya manifiesto, un binario colocado a mano funciona
    let candidatos: [(&str, &str, Vec<String>); 3] = [
        ("LibreDWG dwg2dxf", "dwg2dxf", vec!["-y".into()]),
        ("LibreDWG dwg2dxf", "dwg2dxf.exe", vec!["-y".into()]),
        ("ODA File Converter", "ODAFileConverter.exe", vec![]),
    ];
    for (nombre, fichero, fijos) in candidatos {
        let exe = carpeta.join(fichero);
        if exe.is_file() && !encontrados.iter().any(|c| c.ejecutable.ends_with(fichero)) {
            encontrados.push(ConversorInfo {
                nombre: nombre.into(),
                ejecutable: exe.display().to_string(),
                version: version_de(&exe),
                origen: "colocado manualmente en tools/converter".into(),
                args_fijos: fijos,
            });
        }
    }

    // En Linux/macOS el conversor puede estar en el sistema (PATH) — útil para pruebas
    #[cfg(not(windows))]
    {
        if encontrados.is_empty() {
            if let Ok(salida) = Command::new("which").arg("dwg2dxf").output() {
                if salida.status.success() {
                    let p = String::from_utf8_lossy(&salida.stdout).trim().to_string();
                    if !p.is_empty() {
                        encontrados.push(ConversorInfo {
                            nombre: "LibreDWG dwg2dxf (sistema)".into(),
                            ejecutable: p.clone(),
                            version: version_de(Path::new(&p)),
                            origen: "PATH del sistema".into(),
                            args_fijos: vec!["-y".into()],
                        });
                    }
                }
            }
        }
    }
    encontrados
}

/// Versión del conversor: se pregunta al propio ejecutable (nunca se adivina).
fn version_de(exe: &Path) -> String {
    for bandera in ["--version", "-v"] {
        let (out, err, _) = ejecutar(exe, &[bandera.to_string()]);
        let texto = format!("{} {}", out.trim(), err.trim());
        for linea in texto.lines() {
            let l = linea.trim();
            if !l.is_empty() && l.len() < 120 && !l.to_lowercase().contains("error") && (l.contains(char::is_numeric)) {
                return l.to_string();
            }
        }
    }
    "versión no declarada".into()
}

/// Ejecuta la conversión DWG → DXF y verifica el resultado. Devuelve la traza completa.
pub fn convertir(conv: &ConversorInfo, entrada: &Path, salida: &Path) -> Result<Value, String> {
    if !entrada.is_file() {
        return Err(format!("no existe el archivo de entrada: {}", entrada.display()));
    }
    if let Some(dir) = salida.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(salida); // el conversor no sobrescribe por defecto

    // LibreDWG: `dwg2dxf [-y] -o SALIDA ENTRADA`; ODA: carpeta de entrada, salida, versión, tipo, recursivo, auditoría
    let es_oda = conv.nombre.to_lowercase().contains("oda");
    let mut args: Vec<String> = conv.args_fijos.clone();
    if es_oda {
        let dir_in = entrada.parent().unwrap_or(Path::new(".")).display().to_string();
        let dir_out = salida.parent().unwrap_or(Path::new(".")).display().to_string();
        args.extend([
            dir_in,
            dir_out,
            "ACAD2018".into(),
            "DXF".into(),
            "0".into(),
            "1".into(),
        ]);
    } else {
        args.extend([
            "-o".into(),
            salida.display().to_string(),
            entrada.display().to_string(),
        ]);
    }

    let (out, err, codigo) = ejecutar(Path::new(&conv.ejecutable), &args);
    let existe = salida.is_file();
    let bytes = std::fs::metadata(salida).map(|m| m.len()).unwrap_or(0);
    // El conversor puede escribir el DXF junto al DWG con el mismo nombre: se busca esa salida también
    let alternativa = entrada.with_extension("dxf");
    let (ruta_efectiva, bytes_efectivos) = if existe {
        (salida.display().to_string(), bytes)
    } else if alternativa.is_file() && alternativa != salida {
        let b = std::fs::metadata(&alternativa).map(|m| m.len()).unwrap_or(0);
        // se mueve a la ruta pedida para que la interfaz reciba siempre lo que pidió
        std::fs::rename(&alternativa, salida).map_err(|e| e.to_string())?;
        (salida.display().to_string(), b)
    } else {
        (salida.display().to_string(), 0)
    };

    Ok(json!({
        "ok": existe || bytes_efectivos > 0,
        "conversor": conv.nombre,
        "versiónConversor": conv.version,
        "ejecutable": conv.ejecutable,
        "argumentos": args,
        "entrada": entrada.display().to_string(),
        "salida": ruta_efectiva,
        "bytes": bytes_efectivos,
        "codigoSalida": codigo,
        "salidaEstandar": out.lines().take(25).collect::<Vec<_>>().join("\n"),
        "erroresEstandar": err.lines().take(25).collect::<Vec<_>>().join("\n"),
    }))
}

/// Ruta del conversor en la carpeta del programa (la usa el asistente de la interfaz).
pub fn carpeta_por_defecto(raiz: &Path) -> PathBuf {
    raiz.join("tools").join("converter")
}
