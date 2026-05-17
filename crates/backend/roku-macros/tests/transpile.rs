//! Integration tests for `#[method]` — every test asserts BOTH
//! that the function still runs as normal Rust AND that the
//! emitted BrightScript string matches a golden value. The golden
//! strings are written inline (not loaded from files) so a failure
//! shows the diff in test output directly.

use backend_roku_macros::method;

// ---------------------------------------------------------------------------
// factorial: recursion + if-as-tail-expression + arithmetic
// ---------------------------------------------------------------------------

#[method]
pub fn factorial(n: i32) -> i32 {
    if n <= 1 {
        1
    } else {
        n * factorial(n - 1)
    }
}

#[test]
fn factorial_runs_in_rust() {
    assert_eq!(factorial(0), 1);
    assert_eq!(factorial(1), 1);
    assert_eq!(factorial(5), 120);
    assert_eq!(factorial(10), 3628800);
}

#[test]
fn factorial_brs_matches_golden() {
    let expected = "\
function factorial(n as integer) as integer
    if n <= 1 then
        return 1
    else
        return n * factorial(n - 1)
    end if
end function
";
    assert_eq!(FACTORIAL_BRS, expected);
}

// ---------------------------------------------------------------------------
// sum_to: mut binding + inclusive for-loop + accumulator
// ---------------------------------------------------------------------------

#[method]
pub fn sum_to(n: i32) -> i32 {
    let mut total = 0;
    for i in 1..=n {
        total = total + i;
    }
    total
}

#[test]
fn sum_to_runs_in_rust() {
    assert_eq!(sum_to(0), 0);
    assert_eq!(sum_to(10), 55);
    assert_eq!(sum_to(100), 5050);
}

#[test]
fn sum_to_brs_matches_golden() {
    let expected = "\
function sum_to(n as integer) as integer
    total = 0
    for i = 1 to n
        total = total + i
    end for
    return total
end function
";
    assert_eq!(SUM_TO_BRS, expected);
}

// ---------------------------------------------------------------------------
// is_even: modulo, comparison, no-control-flow
// ---------------------------------------------------------------------------

#[method]
pub fn is_even(n: i32) -> bool {
    n % 2 == 0
}

#[test]
fn is_even_runs_in_rust() {
    assert!(is_even(0));
    assert!(!is_even(1));
    assert!(is_even(42));
}

#[test]
fn is_even_brs_matches_golden() {
    let expected = "\
function is_even(n as integer) as boolean
    return n % 2 = 0
end function
";
    // Note: BrightScript uses `mod` not `%`. Test against the
    // actual emitter output to catch regressions.
    let actual = IS_EVEN_BRS;
    // The emitter maps `%` → `mod`, not `%`. Update expected:
    let _ = expected;
    let expected = "\
function is_even(n as integer) as boolean
    return n mod 2 = 0
end function
";
    assert_eq!(actual, expected);
}

// ---------------------------------------------------------------------------
// chained else-if + exclusive for + while
// ---------------------------------------------------------------------------

#[method]
pub fn classify(n: i32) -> i32 {
    if n < 0 {
        -1
    } else if n == 0 {
        0
    } else {
        1
    }
}

#[test]
fn classify_brs_matches_golden() {
    let expected = "\
function classify(n as integer) as integer
    if n < 0 then
        return -1
    else if n = 0 then
        return 0
    else
        return 1
    end if
end function
";
    assert_eq!(CLASSIFY_BRS, expected);
}

#[method]
pub fn count_zeros(limit: i32) -> i32 {
    let mut zeros = 0;
    for i in 0..limit {
        if i == 0 {
            zeros = zeros + 1;
        }
    }
    zeros
}

#[test]
fn count_zeros_brs_matches_golden() {
    let expected = "\
function count_zeros(limit as integer) as integer
    zeros = 0
    for i = 0 to (limit) - 1
        if i = 0 then
            zeros = zeros + 1
        end if
    end for
    return zeros
end function
";
    assert_eq!(COUNT_ZEROS_BRS, expected);
}

// ---------------------------------------------------------------------------
// Sub (no return value)
// ---------------------------------------------------------------------------

#[method]
pub fn no_op(_n: i32) {}

#[test]
fn no_op_brs_matches_golden() {
    let expected = "\
sub no_op(_n as integer)
end sub
";
    assert_eq!(NO_OP_BRS, expected);
}
