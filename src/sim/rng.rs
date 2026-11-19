//! A tiny seeded PRNG (SplitMix64). We own it so benchmark numbers stay
//! reproducible from a seed regardless of upstream crate changes.

#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [lo, hi). Panics if the range is empty.
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        assert!(lo < hi, "empty range {lo}..{hi}");
        lo + (self.next_u64() % (hi - lo) as u64) as usize
    }

    pub fn between(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.range(0, items.len())]
    }

    /// Picks an index with probability falling off like 1/(i+1), so a few
    /// items are popular and most are not.
    pub fn skewed(&mut self, n: usize) -> usize {
        let total: f64 = (1..=n).map(|i| 1.0 / i as f64).sum();
        let mut x = self.unit() * total;
        for i in 0..n {
            x -= 1.0 / (i + 1) as f64;
            if x <= 0.0 {
                return i;
            }
        }
        n - 1
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.range(0, i + 1);
            items.swap(i, j);
        }
    }

    pub fn sample<T: Clone>(&mut self, items: &[T], k: usize) -> Vec<T> {
        let mut v = items.to_vec();
        self.shuffle(&mut v);
        v.truncate(k);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let a: Vec<u64> = (0..5)
            .map({
                let mut r = Rng::new(42);
                move |_| r.next_u64()
            })
            .collect();
        let mut r = Rng::new(42);
        assert!(a.iter().all(|x| *x == r.next_u64()));
    }

    #[test]
    fn ranges_stay_in_bounds() {
        let mut r = Rng::new(1);
        for _ in 0..1000 {
            assert!((3..7).contains(&r.range(3, 7)));
            assert!((0.0..1.0).contains(&r.unit()));
        }
    }
}
