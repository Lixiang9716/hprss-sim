//! UUniFast-Discard utilization generation (Bini & Buttazzo, 2005).

use rand::Rng;

/// Generate `n` utilization values summing to `total_u`, each in (0, 1].
/// Uses UUniFast-Discard: if any u_i > 1.0, discard and retry.
pub fn uunifast_discard(n: usize, total_u: f64, rng: &mut impl Rng) -> Vec<f64> {
    assert!(n > 0);
    assert!(total_u > 0.0);
    loop {
        let utils = uunifast(n, total_u, rng);
        if utils.iter().all(|&u| u > 0.0 && u <= 1.0) {
            return utils;
        }
    }
}

fn uunifast(n: usize, total_u: f64, rng: &mut impl Rng) -> Vec<f64> {
    let mut result = Vec::with_capacity(n);
    let mut sum_u = total_u;
    for i in 1..n {
        let exp = 1.0 / (n - i) as f64;
        let next_sum = sum_u * rng.gen_range(0.0..1.0_f64).powf(exp);
        result.push(sum_u - next_sum);
        sum_u = next_sum;
    }
    result.push(sum_u);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    #[test]
    fn utilizations_sum_to_target() {
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let utils = uunifast_discard(10, 0.7, &mut rng);
        assert_eq!(utils.len(), 10);
        let sum: f64 = utils.iter().sum();
        assert!((sum - 0.7).abs() < 1e-10);
    }

    #[test]
    fn all_in_range() {
        let mut rng = ChaCha8Rng::seed_from_u64(123);
        let utils = uunifast_discard(20, 0.8, &mut rng);
        for u in &utils {
            assert!(*u > 0.0 && *u <= 1.0);
        }
    }

    #[test]
    fn deterministic_with_seed() {
        let u1 = uunifast_discard(5, 0.5, &mut ChaCha8Rng::seed_from_u64(42));
        let u2 = uunifast_discard(5, 0.5, &mut ChaCha8Rng::seed_from_u64(42));
        assert_eq!(u1, u2);
    }
}
