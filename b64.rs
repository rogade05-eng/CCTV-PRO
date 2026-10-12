/*
 * CCTV DESIGN PRO — Base64 sin dependencias externas (§9)
 *
 * Los ficheros binarios (DWG, DXF binario, ZIP, PDF) no pueden cruzar la frontera
 * interfaz ↔ shell como texto: al convertirlos carácter a carácter, cada byte mayor de 127
 * se transforma en dos bytes UTF-8 y el fichero llega corrompido. Ese era el motivo real de
 * que la conversión DWG → DXF fallara al ejecutarla desde el panel (el modo de consola
 * `--convert`, que trabaja con rutas, nunca se vio afectado).
 *
 * Base64 resuelve el problema: es texto seguro de 7 bits, se puede pasar por JSON y
 * reconstruye los bytes exactamente iguales. Implementación mínima y verificable, sin crates.
 */

const ALFABETO: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Bytes → Base64 (con relleno `=` como exige el estándar).
pub fn codificar(datos: &[u8]) -> String {
    let mut salida = String::with_capacity(datos.len().div_ceil(3) * 4);
    for trozo in datos.chunks(3) {
        let b0 = trozo[0] as u32;
        let b1 = *trozo.get(1).unwrap_or(&0) as u32;
        let b2 = *trozo.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        salida.push(ALFABETO[((n >> 18) & 63) as usize] as char);
        salida.push(ALFABETO[((n >> 12) & 63) as usize] as char);
        if trozo.len() > 1 {
            salida.push(ALFABETO[((n >> 6) & 63) as usize] as char);
        } else {
            salida.push('=');
        }
        if trozo.len() > 2 {
            salida.push(ALFABETO[(n & 63) as usize] as char);
        } else {
            salida.push('=');
        }
    }
    salida
}

fn valor(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a') as u32 + 26),
        b'0'..=b'9' => Some((c - b'0') as u32 + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Base64 → bytes. Ignora saltos de línea y espacios; falla si el texto no es Base64 válido.
pub fn decodificar(texto: &str) -> Result<Vec<u8>, String> {
    let limpio: Vec<u8> = texto
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
        .collect();
    let mut salida = Vec::with_capacity(limpio.len() / 4 * 3 + 3);
    for trozo in limpio.chunks(4) {
        let mut n: u32 = 0;
        for (i, c) in trozo.iter().enumerate() {
            let v = valor(*c).ok_or_else(|| format!("carácter no válido en Base64: {:?}", *c as char))?;
            n |= v << (18 - 6 * i);
        }
        salida.push(((n >> 16) & 255) as u8);
        if trozo.len() > 2 {
            salida.push(((n >> 8) & 255) as u8);
        }
        if trozo.len() > 3 {
            salida.push((n & 255) as u8);
        }
    }
    Ok(salida)
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn ida_y_vuelta() {
        for caso in [
            vec![],
            vec![0u8],
            vec![1, 2],
            vec![1, 2, 3],
            (0..=255u8).collect::<Vec<u8>>(),
            vec![77, 80, 0, 255, 128, 10, 13],
        ] {
            let codificado = codificar(&caso);
            let vuelta = decodificar(&codificado).unwrap();
            assert_eq!(caso, vuelta, "falló con {} bytes", caso.len());
        }
    }

    #[test]
    fn valores_conocidos() {
        assert_eq!(codificar(b"Man"), "TWFu");
        assert_eq!(codificar(b"Ma"), "TWE=");
        assert_eq!(codificar(b"M"), "TQ==");
        assert_eq!(decodificar("TWFu").unwrap(), b"Man".to_vec());
        assert_eq!(decodificar(" TWFu\n").unwrap(), b"Man".to_vec());
    }

    #[test]
    fn entrada_no_valida() {
        assert!(decodificar("TWF*").is_err());
    }
}
