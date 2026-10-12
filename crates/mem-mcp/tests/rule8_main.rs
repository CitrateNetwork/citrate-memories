//! Rule 8 tripwire for the mem-mcp daemon binary: no `.unwrap()` / `.expect(` in
//! `src/main.rs`. A daemon that panics on a startup failure gives citrate-core
//! nothing to show the member; every failure path prints a reason and exits.

#[test]
fn main_rs_has_no_unwrap_or_expect() {
    let src = include_str!("../src/main.rs");
    let unwrap = [".unwrap", "()"].concat();
    let expect = [".expect", "("].concat();
    let hits: Vec<(usize, &str)> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .filter(|(_, l)| l.contains(&unwrap) || l.contains(&expect))
        .map(|(i, l)| (i + 1, l.trim()))
        .collect();
    assert!(
        hits.is_empty(),
        "Rule 8: unwrap/expect in mem-mcp main.rs: {hits:?}"
    );
}
