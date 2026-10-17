//! `Ref` fills are leased: the element that filled a ref releases the
//! fill when it tears down, so the ref reads `None` once it's gone — and
//! releasing an older fill never clears a newer one.

use runtime_shared::Ref;

#[test]
fn releasing_a_fill_empties_the_ref() {
    let r: Ref<u32> = Ref::new();
    let lease = r.fill_owned(7);
    assert_eq!(r.get(), Some(7));
    lease.release();
    assert_eq!(r.get(), None);
    assert!(!r.is_mounted());
}

/// A branch can remount an element bearing the same ref before the old
/// one tears down. The old element's release must leave the new handle.
#[test]
fn releasing_an_older_fill_keeps_the_newer_one() {
    let r: Ref<u32> = Ref::new();
    let old = r.fill_owned(1);
    let new = r.fill_owned(2);
    old.release();
    assert_eq!(r.get(), Some(2), "the newer fill survives the older one's release");
    new.release();
    assert_eq!(r.get(), None);
}

#[test]
fn a_release_after_clear_and_refill_leaves_the_refill() {
    let r: Ref<u32> = Ref::new();
    let lease = r.fill_owned(1);
    r.clear();
    r.fill(3);
    lease.release();
    assert_eq!(r.get(), Some(3));
}
