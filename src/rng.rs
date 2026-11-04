//! Seeded, platform-independent random numbers.
//!
//! Everything in the crate that needs randomness (net-order shuffling, annealing moves)
//! draws it from an [`Rng`] created from a caller-supplied seed. There is no thread or OS
//! seeding and no global state, so a seed fully determines a run.
//!
//! The generator is xoshiro256++ by David Blackman and Sebastiano Vigna, with its state
//! filled from the seed by SplitMix64, as its authors recommend for 64-bit seeds. It was
//! chosen because it is small (32 bytes of state, a few shifts per draw), passes the
//! standard statistical test suites, has a period of 2^256 - 1, and has a reference
//! implementation with published output to test against. It is not cryptographically
//! secure, and nothing here needs it to be.

/// A seeded random number generator.
///
/// The same seed gives the same sequence on every platform. Every value is derived from
/// whole `u64` draws, and neither the number of draws nor the values depend on the width
/// of `usize`. Per the crate's determinism rules, a seed and a crate version together
/// identify a sequence: changing the output would be deliberate, and tests catch it if
/// not, but stability across versions is not promised.
///
/// `Rng` is [`Clone`] so a caller can save and replay a stream, but deliberately not
/// `Copy`, because an implicit copy would silently repeat draws.
///
/// # Examples
///
/// ```
/// use guide::rng::Rng;
///
/// let mut a = Rng::new(42);
/// let mut b = a.clone();
/// assert_eq!(a.next_u64(), b.next_u64());
///
/// let die = a.below(6);
/// assert!(die < 6);
///
/// let probability = a.unit();
/// assert!((0.0..1.0).contains(&probability));
///
/// let mut nets = [10, 20, 30, 40];
/// a.shuffle(&mut nets);
/// nets.sort_unstable();
/// assert_eq!(nets, [10, 20, 30, 40]);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    state: [u64; 4],
}

impl Rng {
    /// Creates a generator from a seed. Every seed is valid, including zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use guide::rng::Rng;
    ///
    /// assert_eq!(Rng::new(7).next_u64(), Rng::new(7).next_u64());
    /// assert_ne!(Rng::new(7).next_u64(), Rng::new(8).next_u64());
    /// ```
    pub fn new(seed: u64) -> Rng {
        // xoshiro256++ must not start from an all-zero state, and this cannot produce one:
        // SplitMix64 outputs a bijection of a counter, so it gives zero for at most one
        // counter value and never for four in a row.
        let mut counter = seed;
        Rng {
            state: [
                splitmix64(&mut counter),
                splitmix64(&mut counter),
                splitmix64(&mut counter),
                splitmix64(&mut counter),
            ],
        }
    }

    /// Returns the next 64 uniformly distributed bits.
    ///
    /// # Examples
    ///
    /// ```
    /// use guide::rng::Rng;
    ///
    /// let mut rng = Rng::new(1);
    /// assert_ne!(rng.next_u64(), rng.next_u64());
    /// ```
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let [s0, s1, s2, s3] = self.state;
        let result = s0.wrapping_add(s3).rotate_left(23).wrapping_add(s0);

        let t = s1 << 17;
        let s2 = s2 ^ s0;
        let s3 = s3 ^ s1;
        let s1 = s1 ^ s2;
        let s0 = s0 ^ s3;
        let s2 = s2 ^ t;
        let s3 = s3.rotate_left(45);

        self.state = [s0, s1, s2, s3];
        result
    }

    /// Returns a uniformly distributed integer in `0..n`, with no modulo bias.
    ///
    /// The range is empty when `n` is zero, so this returns `0` without using a draw,
    /// rather than panicking. Callers that can pass zero and care about the difference
    /// should check first. For any other `n` the result is below `n`, and each call uses
    /// one draw, plus another for each rare rejection (see below), so `below(1)` uses
    /// a draw even though its result is always zero.
    ///
    /// The method is Lemire's nearly divisionless multiply-and-reject (2019). The high half
    /// of `draw * n` maps the draw onto the range, but unevenly unless `n` divides 2^64, so
    /// a draw whose low half is below `2^64 mod n` is rejected and replaced. All of this
    /// happens in `u64` and `u128`, so the same draws are accepted on every platform.
    ///
    /// # Examples
    ///
    /// ```
    /// use guide::rng::Rng;
    ///
    /// let mut rng = Rng::new(3);
    /// assert!(rng.below(10) < 10);
    /// assert_eq!(rng.below(1), 0);
    /// assert_eq!(rng.below(0), 0);
    /// ```
    #[inline]
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let mut product = u128::from(self.next_u64()) * u128::from(n);
        // The rejection threshold, 2^64 mod n, is always below n, so the division can be
        // skipped unless the low half is below n too.
        if (product as u64) < n {
            let threshold = n.wrapping_neg() % n;
            while (product as u64) < threshold {
                product = u128::from(self.next_u64()) * u128::from(n);
            }
        }
        (product >> 64) as u64
    }

    /// Returns a uniformly distributed `f64` in `[0, 1)`.
    ///
    /// The result is a whole multiple of 2^-53, taken from the top 53 bits of one draw.
    /// Both the conversion and the scaling are exact, so nothing rounds and the bits do
    /// not depend on the platform. Every call uses exactly one draw.
    ///
    /// # Examples
    ///
    /// ```
    /// use guide::rng::Rng;
    ///
    /// let mut rng = Rng::new(5);
    /// let value = rng.unit();
    /// assert!(0.0 <= value && value < 1.0);
    /// ```
    #[inline]
    pub fn unit(&mut self) -> f64 {
        // 2^-53, as an exponent field of 1023 - 53.
        const SCALE: f64 = f64::from_bits(970 << 52);
        (self.next_u64() >> 11) as f64 * SCALE
    }

    /// Shuffles `items` in place, with every permutation equally likely.
    ///
    /// This is the Fisher-Yates shuffle, walking from the end. A slice of `len` items uses
    /// one [`below`](Rng::below) call for each of its last `len - 1` positions, so
    /// empty and single-item slices use no draws.
    ///
    /// # Examples
    ///
    /// ```
    /// use guide::rng::Rng;
    ///
    /// let mut order = [0, 1, 2, 3, 4];
    /// Rng::new(9).shuffle(&mut order);
    /// order.sort_unstable();
    /// assert_eq!(order, [0, 1, 2, 3, 4]);
    /// ```
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            // Widening to u64 is lossless everywhere, and the same value reaches `below`
            // on 32-bit and 64-bit targets. The result is at most i, so it fits back.
            let j = self.below(i as u64 + 1) as usize;
            items.swap(i, j);
        }
    }
}

/// Advances a SplitMix64 counter and returns its next output.
fn splitmix64(counter: &mut u64) -> u64 {
    *counter = counter.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *counter;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    // Named rather than a glob, because proptest's prelude has a trait called `Rng`.
    use super::{Rng, splitmix64};
    use proptest::prelude::*;

    /// A generator with a given raw state, to reach the published vector for xoshiro256++,
    /// which starts from a state rather than from a seed.
    fn from_state(state: [u64; 4]) -> Rng {
        Rng { state }
    }

    /// Pearson's statistic for counts that should be equal.
    fn chi_squared(counts: &[u32]) -> f64 {
        let total: f64 = counts.iter().map(|&c| f64::from(c)).sum();
        let expected = total / counts.len() as f64;
        counts
            .iter()
            .map(|&c| (f64::from(c) - expected) * (f64::from(c) - expected) / expected)
            .sum()
    }

    /// Reference vector for SplitMix64: the sequence from seed 1234567 published as the
    /// test values of Rosetta Code's "Pseudo-random numbers/Splitmix64" task.
    #[test]
    fn splitmix64_matches_published_vector() {
        let mut counter = 1_234_567;
        for expected in [
            6_457_827_717_110_365_317,
            3_203_168_211_198_807_973,
            9_817_491_932_198_370_423,
            4_593_380_528_125_082_431,
            16_408_922_859_458_223_821,
        ] {
            assert_eq!(splitmix64(&mut counter), expected);
        }
    }

    /// Reference vector for xoshiro256++: output from state [1, 2, 3, 4], from the
    /// `reference` test in `rand` 0.8.5 (`src/rngs/xoshiro256plusplus.rs`), which says the
    /// values come from the authors' `xoshiro256plusplus.c`.
    #[test]
    fn xoshiro256plusplus_matches_published_vector() {
        let mut rng = from_state([1, 2, 3, 4]);
        for expected in [
            41_943_041,
            58_720_359,
            3_588_806_011_781_223,
            3_591_011_842_654_386,
            9_228_616_714_210_784_205,
            9_973_669_472_204_895_162,
            14_011_001_112_246_962_877,
            12_406_186_145_184_390_807,
            15_849_039_046_786_891_736,
            10_450_023_813_501_588_000,
        ] {
            assert_eq!(rng.next_u64(), expected);
        }
    }

    /// Regression values, not published vectors: these came from this implementation. They
    /// pin how a seed becomes a stream, which the reference vectors above do not cover.
    #[test]
    fn seeded_streams_match_regression_values() {
        let cases = [
            (
                0,
                [
                    0x5317_5D61_490B_23DF,
                    0x61DA_6F3D_C380_D507,
                    0x5C0F_DF91_EC9A_7BFC,
                    0x02EE_BF8C_3BBE_5E1A,
                    0x7ECA_04EB_AF4A_5EEA,
                ],
            ),
            (
                1,
                [
                    0xCFC5_D07F_6F03_C29B,
                    0xBF42_4132_963F_E08D,
                    0x19A3_7D57_57AA_F520,
                    0xBF08_119F_05CD_56D6,
                    0x2F47_184B_8618_6FA4,
                ],
            ),
            (
                42,
                [
                    0xD076_4D4F_4476_689F,
                    0x519E_4174_576F_3791,
                    0xFBE0_7CFB_0C24_ED8C,
                    0xB37D_9F60_0CD8_35B8,
                    0xCB23_1C38_7484_6A73,
                ],
            ),
            (
                u64::MAX,
                [
                    0x56CC_F8CE_948E_27B2,
                    0xE685_8843_2E5A_5B90,
                    0xE3E9_B5A4_8119_CA8B,
                    0x460F_1949_5532_AE73,
                    0xA7D6_2040_EA92_63E1,
                ],
            ),
        ];
        for (seed, expected) in cases {
            let mut rng = Rng::new(seed);
            assert_eq!(expected.map(|_| rng.next_u64()), expected, "seed {seed}");
        }
    }

    /// Regression values. These would differ on a platform where `below` or `shuffle`
    /// depended on the width of `usize`, so they are worth running on a 32-bit target.
    #[test]
    fn derived_values_match_regression_values() {
        let mut rng = Rng::new(42);
        let below: [u64; 10] = core::array::from_fn(|_| rng.below(1000));
        assert_eq!(below, [814, 318, 983, 701, 793, 588, 125, 605, 207, 933]);

        // A bound beyond 32 bits, whose results would be truncated by any narrowing.
        let mut rng = Rng::new(42);
        let wide: [u64; 5] = core::array::from_fn(|_| rng.below(3 << 62));
        assert_eq!(
            wide,
            [
                0x9C58_B9FB_7358_CE77,
                0x3D36_B117_4193_69AC,
                0x985A_552A_5763_4FD6,
                0x70EA_3740_3ABC_A6DD,
                0x1811_52BF_5993_A800,
            ]
        );

        let mut rng = Rng::new(42);
        let unit: [u64; 5] = core::array::from_fn(|_| rng.unit().to_bits());
        assert_eq!(
            unit,
            [
                0x3FEA_0EC9_A9E8_8ECD,
                0x3FD4_6790_5D15_DBCC,
                0x3FEF_7C0F_9F61_849D,
                0x3FE6_6FB3_EC01_9B06,
                0x3FE9_6463_870E_908D,
            ]
        );

        // The draw after the shuffle pins how many draws the shuffle used.
        let mut rng = Rng::new(42);
        let mut items: [u32; 16] = core::array::from_fn(|i| i as u32);
        rng.shuffle(&mut items);
        assert_eq!(
            items,
            [7, 8, 0, 2, 14, 3, 10, 11, 5, 1, 6, 12, 9, 15, 4, 13]
        );
        assert_eq!(rng.next_u64(), 0x8BC6_E858_D050_1168);
    }

    #[test]
    fn clone_continues_the_same_stream() {
        let mut original = Rng::new(11);
        original.next_u64();
        let mut copy = original.clone();
        assert_eq!(original, copy);
        for _ in 0..100 {
            assert_eq!(original.next_u64(), copy.next_u64());
        }
    }

    #[test]
    fn below_edge_cases() {
        let mut rng = Rng::new(1);

        // An empty range gives zero and leaves the stream alone.
        let before = rng.clone();
        assert_eq!(rng.below(0), 0);
        assert_eq!(rng, before);

        // A range of one value still uses a draw, like any other bound.
        let mut expected = rng.clone();
        expected.next_u64();
        assert_eq!(rng.below(1), 0);
        assert_eq!(rng, expected);

        // The largest bound and a power of two, where the threshold is 1 and 0.
        for _ in 0..1000 {
            assert!(rng.below(u64::MAX) < u64::MAX);
            assert!(rng.below(1 << 63) < 1 << 63);
        }
    }

    /// With a bound of 3 * 2^62, a quarter of the draws must be rejected, which exercises
    /// the loop. `%` would make the lowest third of the range twice as likely as the rest,
    /// so this also tells an unbiased method from a modulo one.
    #[test]
    fn below_is_unbiased_when_rejection_is_common() {
        const N: u64 = 3 << 62;
        const SAMPLES: u32 = 300_000;

        let mut rng = Rng::new(2);
        let mut counts = [0u32; 3];
        for _ in 0..SAMPLES {
            counts[(rng.below(N) >> 62) as usize] += 1;
        }
        assert!(chi_squared(&counts) < 30.0, "{counts:?}");

        // 300,000 calls should use about 400,000 draws.
        let mut rng = Rng::new(3);
        let mut draws = 0u64;
        for _ in 0..SAMPLES {
            let mut probe = rng.clone();
            let result = rng.below(N);
            loop {
                draws += 1;
                let product = u128::from(probe.next_u64()) * u128::from(N);
                if product as u64 >= 1 << 62 {
                    assert_eq!((product >> 64) as u64, result);
                    break;
                }
            }
            assert_eq!(probe, rng);
        }
        assert!((395_000..405_000).contains(&draws), "{draws} draws");
    }

    /// The statistical tests use fixed seeds, so they cannot fail by chance. Their limits
    /// are near the 1e-6 tail of the chi-squared distribution.
    #[test]
    fn below_is_uniform_for_small_bounds() {
        let mut rng = Rng::new(4);

        let mut counts = [0u32; 7];
        for _ in 0..700_000 {
            counts[rng.below(7) as usize] += 1;
        }
        assert!(chi_squared(&counts) < 40.0, "{counts:?}");

        let mut counts = [0u32; 64];
        for _ in 0..640_000 {
            counts[rng.below(64) as usize] += 1;
        }
        assert!(chi_squared(&counts) < 125.0, "{counts:?}");
    }

    #[test]
    fn unit_is_uniform() {
        const SAMPLES: u32 = 1_000_000;

        let mut rng = Rng::new(5);
        let mut counts = [0u32; 32];
        let (mut sum, mut sum_of_squares) = (0.0, 0.0);
        for _ in 0..SAMPLES {
            let value = rng.unit();
            assert!((0.0..1.0).contains(&value));
            counts[(value * 32.0) as usize] += 1;
            sum += value;
            sum_of_squares += value * value;
        }
        assert!(chi_squared(&counts) < 80.0, "{counts:?}");

        // A uniform variable has mean 1/2 and second moment 1/3. Standard errors are 2.9e-4
        // and 3.0e-4 here, so these limits are about five of them.
        let mean = sum / f64::from(SAMPLES);
        let second_moment = sum_of_squares / f64::from(SAMPLES);
        assert!((mean - 0.5).abs() < 1.5e-3, "{mean}");
        assert!(
            (second_moment - 1.0 / 3.0).abs() < 1.5e-3,
            "{second_moment}"
        );
    }

    /// The lowest bit of `unit` is where a conversion that lost bits would show.
    #[test]
    fn unit_uses_all_53_bits() {
        let mut rng = Rng::new(6);
        let mut odd = 0u32;
        for _ in 0..100_000 {
            let scaled = rng.unit() * 9_007_199_254_740_992.0;
            assert_eq!(scaled, scaled as u64 as f64, "must be a multiple of 2^-53");
            odd += (scaled as u64 & 1) as u32;
        }
        assert!((49_000..51_000).contains(&odd), "{odd}");
    }

    #[test]
    fn shuffle_reaches_every_permutation_evenly() {
        let mut rng = Rng::new(7);
        let mut counts = [0u32; 24];
        for _ in 0..240_000 {
            let mut items = [0usize, 1, 2, 3];
            rng.shuffle(&mut items);

            // The Lehmer code of the permutation, as an index below 4!.
            let mut index = 0;
            for (i, &item) in items.iter().enumerate() {
                let smaller_later = items[i + 1..].iter().filter(|&&x| x < item).count();
                index = index * (4 - i) + smaller_later;
            }
            counts[index] += 1;
        }
        assert!(chi_squared(&counts) < 70.0, "{counts:?}");
    }

    #[test]
    fn shuffle_of_short_slices_uses_no_draws() {
        let mut rng = Rng::new(8);
        let before = rng.clone();

        let mut none: [u8; 0] = [];
        rng.shuffle(&mut none);
        let mut one = [1];
        rng.shuffle(&mut one);

        assert_eq!(one, [1]);
        assert_eq!(rng, before);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]

        #[test]
        fn below_stays_in_range(seed: u64, n in 1u64..) {
            let mut rng = Rng::new(seed);
            for _ in 0..16 {
                prop_assert!(rng.below(n) < n);
            }
        }

        #[test]
        fn below_stays_in_range_for_small_bounds(seed: u64, n in 1u64..100) {
            let mut rng = Rng::new(seed);
            for _ in 0..16 {
                prop_assert!(rng.below(n) < n);
            }
        }

        /// Checks `below` against the definition of the method: take the first draw whose
        /// low half is at least 2^64 mod n. Unlike `below`, this always divides.
        #[test]
        fn below_rejects_exactly_the_biased_draws(seed: u64, n in 1u64..) {
            let threshold = ((1u128 << 64) % u128::from(n)) as u64;
            let mut rng = Rng::new(seed);
            let mut probe = rng.clone();
            let result = rng.below(n);
            let expected = loop {
                let product = u128::from(probe.next_u64()) * u128::from(n);
                if product as u64 >= threshold {
                    break (product >> 64) as u64;
                }
            };
            prop_assert_eq!(result, expected);
            prop_assert_eq!(rng, probe);
        }

        #[test]
        fn unit_stays_in_range(seed: u64) {
            let mut rng = Rng::new(seed);
            for _ in 0..16 {
                let value = rng.unit();
                prop_assert!((0.0..1.0).contains(&value));
            }
        }

        #[test]
        fn shuffle_is_a_permutation(seed: u64, items in prop::collection::vec(any::<u8>(), 0..200)) {
            let mut shuffled = items.clone();
            Rng::new(seed).shuffle(&mut shuffled);
            shuffled.sort_unstable();
            let mut sorted = items;
            sorted.sort_unstable();
            prop_assert_eq!(shuffled, sorted);
        }

        /// A shuffle is exactly its sequence of `below` calls, with bounds that come from
        /// the slice length alone, so it leaves the stream in a platform-independent state.
        #[test]
        fn shuffle_is_its_sequence_of_below_calls(seed: u64, len in 0usize..200) {
            let mut rng = Rng::new(seed);
            let mut actual: Vec<usize> = (0..len).collect();
            rng.shuffle(&mut actual);

            let mut probe = Rng::new(seed);
            let mut expected: Vec<usize> = (0..len).collect();
            for i in (1..len as u64).rev() {
                let j = probe.below(i + 1);
                expected.swap(i as usize, j as usize);
            }
            prop_assert_eq!(actual, expected);
            prop_assert_eq!(rng, probe);
        }

        #[test]
        fn seeding_never_gives_the_all_zero_state(seed: u64) {
            prop_assert_ne!(Rng::new(seed).state, [0; 4]);
        }
    }
}
