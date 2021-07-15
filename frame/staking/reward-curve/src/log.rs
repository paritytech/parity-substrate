use std::convert::TryInto;

/// Simple u32 power of 2 function - simply uses a bit shift
macro_rules! pow2 {
	($n:expr) => {
		1_u32 << $n
	}
}

/// Returns the k_th per_million taylor term for a log2 function
fn taylor_term(k: u32, y_num: u128, y_den: u128) -> u32 {
	let _2_div_ln_2: u128 = 2_885_390u128;

	if k == 0 {
		(_2_div_ln_2 * (y_num).pow(1) / (y_den).pow(1)).try_into().unwrap()
	} else {
		let mut res = _2_div_ln_2 * (y_num).pow(3) / (y_den).pow(3);
		for _ in 1..k {
			res = res * (y_num).pow(2) / (y_den).pow(2);
		}
		res /= 2 * k as u128 + 1;

		res.try_into().unwrap()
	}
}

/// Performs a log2 operation using a rational fraction
///
/// result = log2(p/q) where p/q is bound to [1, 1_000_000]
/// Where:
/// * q represents the numerator of the rational fraction input
/// * p represents the denominator of the rational fraction input
/// * result represents a per-million output of log2
pub fn log2(p: u32, q: u32) -> u32 {
	assert!(p >= q); // keep p/q bound to [1, inf)
	assert!(p <= u32::MAX/2);

	// This restriction should not be mandatory. But function is only tested and used for this.
	assert!(p <= 1_000_000);
	assert!(q <= 1_000_000);

	// log2(1) = 0
	if p == q {
		return 0
	}

	// find the power of 2 where q * 2^n <= p < q * 2^(n+1)
	let mut n = 0u32;
	while (p < pow2!(n) * q) || (p >= pow2!(n + 1) * q) {
		n += 1;
		assert!(n < 32); // cannot represent 2^32 in u32
	}
	assert!(p < pow2!(n + 1) * q);

	let y_num: u32 = p - pow2!(n) * q;
	let y_den: u32 = p + pow2!(n) * q;

	// Loop through each Taylor series coefficient until it reaches 10^-6
	let mut res = n * 1_000_000u32;
	let mut k = 0;
	loop {
		let term = taylor_term(k, y_num.into(), y_den.into());
		if term == 0 {
			break
		}

		res += term;
		k += 1;
	}

	res
}

#[test]
fn test_log() {
	let div = 1_000;
	for p in 0..=div {
		for q in 1..=p {
			let p: u32 = (1_000_000 as u64 * p as u64 / div as u64).try_into().unwrap();
			let q: u32 = (1_000_000 as u64 * q as u64 / div as u64).try_into().unwrap();

			let res = - (log2(p, q) as i64);
			let expected = ((q as f64 / p as f64).log(2.0) * 1_000_000 as f64).round() as i64;
			assert!((res - expected).abs() <= 6);
		}
	}
}

#[test]
#[should_panic]
fn test_log_p_must_be_greater_than_q() {
	let p: u32 = 1_000;
	let q: u32 = 1_001;
	let _ = log2(p, q);
}

#[test]
#[should_panic]
fn test_log_p_upper_bound() {
	let p: u32 = 1_000_001;
	let q: u32 = 1_000_000;
	let _ = log2(p, q);
}

#[test]
#[should_panic]
fn test_log_q_limit() {
	let p: u32 = 1_000_000;
	let q: u32 = 0;
	let _ = log2(p, q);
}

#[test]
fn test_log_of_one_boundary() {
	let p: u32 = 1_000_000;
	let q: u32 = 1_000_000;
	assert_eq!(log2(p, q), 0);
}

#[test]
fn test_log_of_largest_input() {
	let p: u32 = 1_000_000;
	let q: u32 = 1;
	let expected = 19_931_568;
	let tolerance = 100;
	assert!((log2(p, q) as i32 - expected as i32).abs() < tolerance);
}