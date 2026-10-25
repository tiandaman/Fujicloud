//! Brute-force vector search over every photo's embedding.
//!
//! Embeddings are stored as 8-bit values with one scale per photo (768 dims
//! → 772 bytes), so 100k photos take ~75 MB in memory instead of ~300 MB,
//! with no visible change in ranking. Scoring 100k photos is a ~20 ms scan,
//! fast enough that no approximate index (and its tuning) is needed.

/// Packs an L2-normalised vector as `[scale: f32 LE][i8; dim]`.
pub fn quantize(v: &[f32]) -> Vec<u8> {
    let max = v.iter().fold(0f32, |m, x| m.max(x.abs())).max(1e-12);
    let scale = max / 127.0;
    let mut out = Vec::with_capacity(4 + v.len());
    out.extend_from_slice(&scale.to_le_bytes());
    out.extend(
        v.iter()
            .map(|x| (x / scale).round().clamp(-127.0, 127.0) as i8 as u8),
    );
    out
}

pub fn dequantize(blob: &[u8]) -> Option<Vec<f32>> {
    let scale = f32::from_le_bytes(blob.get(..4)?.try_into().ok()?);
    Some(blob[4..].iter().map(|&b| b as i8 as f32 * scale).collect())
}

#[derive(Default)]
pub struct Index {
    ids: Vec<i64>,
    scales: Vec<f32>,
    dim: usize,
    data: Vec<i8>,
    /// Row of each photo id.
    rows: std::collections::HashMap<i64, usize>,
}

impl Index {
    /// Builds from `(photo id, quantized blob)` rows as stored in the library.
    pub fn new(rows: Vec<(i64, Vec<u8>)>) -> Self {
        let dim = rows.first().map_or(0, |(_, b)| b.len().saturating_sub(4));
        let mut index = Self {
            dim,
            ids: Vec::with_capacity(rows.len()),
            scales: Vec::with_capacity(rows.len()),
            data: Vec::with_capacity(rows.len() * dim),
            rows: std::collections::HashMap::with_capacity(rows.len()),
        };
        for (id, blob) in rows {
            if blob.len() != dim + 4 {
                continue;
            }
            index.rows.insert(id, index.ids.len());
            index.ids.push(id);
            index
                .scales
                .push(f32::from_le_bytes(blob[..4].try_into().unwrap()));
            index.data.extend(blob[4..].iter().map(|&b| b as i8));
        }
        index
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn contains(&self, id: i64) -> bool {
        self.rows.contains_key(&id)
    }

    pub fn vector(&self, id: i64) -> Option<Vec<f32>> {
        let i = *self.rows.get(&id)?;
        let row = &self.data[i * self.dim..(i + 1) * self.dim];
        Some(row.iter().map(|&q| q as f32 * self.scales[i]).collect())
    }

    /// Cosine similarity of two photos' vectors.
    pub fn similarity(&self, a: i64, b: i64) -> Option<f32> {
        let (i, j) = (*self.rows.get(&a)?, *self.rows.get(&b)?);
        let row = |k: usize| &self.data[k * self.dim..(k + 1) * self.dim];
        let dot: i32 = row(i)
            .iter()
            .zip(row(j))
            .map(|(&x, &y)| x as i32 * y as i32)
            .sum();
        Some(dot as f32 * self.scales[i] * self.scales[j])
    }

    /// The `limit` best matches for `query`, best first, keeping only those
    /// within `spread` of the best score: a vague query returns few photos
    /// rather than the whole library in some order.
    pub fn search(&self, query: &[f32], limit: usize, spread: f32) -> Vec<(i64, f32)> {
        if query.len() != self.dim || self.dim == 0 {
            return Vec::new();
        }
        let mut scored: Vec<(i64, f32)> = self
            .data
            .chunks_exact(self.dim)
            .zip(&self.ids)
            .zip(&self.scales)
            .map(|((row, &id), &scale)| (id, dot(row, query) * scale))
            .collect();
        let limit = limit.min(scored.len());
        if limit == 0 {
            return Vec::new();
        }
        scored.select_nth_unstable_by(limit - 1, |a, b| b.1.total_cmp(&a.1));
        scored.truncate(limit);
        scored.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        let best = scored[0].1;
        scored.retain(|(_, s)| *s >= best - spread);
        scored
    }
}

fn dot(row: &[i8], query: &[f32]) -> f32 {
    // Four accumulators so the compiler can vectorise.
    let mut acc = [0f32; 4];
    let (cr, rr) = row.as_chunks::<4>();
    let (cq, rq) = query.as_chunks::<4>();
    for (x, y) in cr.iter().zip(cq) {
        for k in 0..4 {
            acc[k] += x[k] as f32 * y[k];
        }
    }
    acc.iter().sum::<f32>() + rr.iter().zip(rq).map(|(&x, y)| x as f32 * y).sum::<f32>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: &[f32]) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    #[test]
    fn quantization_round_trips_closely() {
        let v = unit(&[0.3, -0.1, 0.05, 0.9, -0.2]);
        let back = dequantize(&quantize(&v)).unwrap();
        for (a, b) in v.iter().zip(&back) {
            assert!((a - b).abs() < 0.01, "{a} vs {b}");
        }
    }

    #[test]
    fn ranks_by_similarity_and_trims_the_tail() {
        let rows = [
            (1, unit(&[1.0, 0.0, 0.0, 0.0, 0.0])),
            (2, unit(&[0.8, 0.6, 0.0, 0.0, 0.0])),
            (3, unit(&[0.0, 1.0, 0.0, 0.0, 0.0])),
            (4, unit(&[0.0, 0.0, 1.0, 0.0, 0.0])),
        ];
        let index = Index::new(rows.iter().map(|(id, v)| (*id, quantize(v))).collect());
        assert_eq!(index.len(), 4);
        let q = [1.0, 0.0, 0.0, 0.0, 0.0];
        let hits = index.search(&q, 10, 0.5);
        assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), vec![1, 2]);
        assert!((hits[0].1 - 1.0).abs() < 0.01);
        assert_eq!(index.search(&q, 1, 1.0).len(), 1);
        assert!(
            index.search(&[1.0, 0.0], 10, 1.0).is_empty(),
            "dimension mismatch"
        );
        let v3 = index.vector(3).unwrap();
        assert!((v3[1] - 1.0).abs() < 0.01);
    }
}
