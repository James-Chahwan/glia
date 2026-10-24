//! The vector codec and the similarity every glia reader ranks by (FB.1).
//!
//! A stored vector is `dims` IEEE-754 f32 values, little-endian, in index
//! order: the contract agreed with Engram (f32 little-endian). Vectors are
//! L2-normalised before they are stored, so similarity is the raw dot product
//! of two unit vectors ([`cosine`]), accumulated in f64 in index order so the
//! sum is the same on every machine.

/// Encodes `v` as little-endian f32 bytes, `4 * v.len()` long.
pub fn to_le_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Decodes `dims` little-endian f32 values. Err when `bytes` is not exactly
/// `dims * 4` long or a value is NaN or infinite.
pub fn from_le_bytes(bytes: &[u8], dims: u32) -> Result<Vec<f32>, String> {
    let want = (dims as usize)
        .checked_mul(4)
        .ok_or_else(|| format!("vector of {dims} dims is too large"))?;
    if bytes.len() != want {
        return Err(format!(
            "vector is {} bytes, expected {want} ({dims} dims of f32)",
            bytes.len()
        ));
    }
    let mut out = Vec::with_capacity(dims as usize);
    for (i, chunk) in bytes.chunks_exact(4).enumerate() {
        let x = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        if !x.is_finite() {
            return Err(format!("vector value {i} is not finite ({x})"));
        }
        out.push(x);
    }
    Ok(out)
}

/// The Euclidean norm of `v`, accumulated in f64 in index order.
pub fn norm(v: &[f32]) -> f64 {
    v.iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        .sqrt()
}

/// Scales `v` to unit length in place. A zero vector is left as it is.
pub fn l2_normalise(v: &mut [f32]) {
    let n = norm(v);
    if n > 0.0 {
        for x in v.iter_mut() {
            *x = (f64::from(*x) / n) as f32;
        }
    }
}

/// The cosine of two unit vectors: `sum a_i * b_i` over the shared indices,
/// in index order, accumulated in f64. Vectors of different lengths compare
/// over the shorter one's indices (a reader checks dims before comparing).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(&x, &y)| f64::from(x) * f64::from(y))
        .sum::<f64>() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_is_exact() {
        let v = vec![0.0f32, -1.5, 3.25e-7, f32::MIN_POSITIVE, 1.0];
        let bytes = to_le_bytes(&v);
        assert_eq!(bytes.len(), 20);
        assert_eq!(&bytes[4..8], &(-1.5f32).to_le_bytes());
        let back = from_le_bytes(&bytes, 5).unwrap();
        assert_eq!(
            back.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
            v.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn decode_refuses_bad_input() {
        assert!(from_le_bytes(&[0u8; 7], 2).unwrap_err().contains("7 bytes"));
        let bad = to_le_bytes(&[1.0, f32::NAN]);
        assert!(from_le_bytes(&bad, 2).unwrap_err().contains("not finite"));
        let inf = to_le_bytes(&[f32::INFINITY]);
        assert!(from_le_bytes(&inf, 1).is_err());
    }

    #[test]
    fn normalise_and_cosine() {
        let mut v = vec![3.0f32, 4.0];
        l2_normalise(&mut v);
        assert!((norm(&v) - 1.0).abs() < 1e-6);
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
        assert!((cosine(&v, &[0.6, 0.8]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-9);
        let mut z = vec![0.0f32; 3];
        l2_normalise(&mut z);
        assert_eq!(z, vec![0.0; 3]);
    }
}
