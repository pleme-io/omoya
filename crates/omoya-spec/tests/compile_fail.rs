//! The seals: every illegal state in `theory/OMOYA.md`'s M0 table, proven to be
//! a COMPILE error rather than a runtime check.
//!
//! Each case's committed `.stderr` is the receipt. If one of these starts
//! compiling, an invariant has been lost — which is precisely the event a
//! runtime test cannot notice.
#[test]
fn illegal_states_do_not_compile() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
