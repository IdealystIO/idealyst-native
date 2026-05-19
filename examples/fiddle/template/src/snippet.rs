//! Auto-generated per /compile request. Do not edit by hand —
//! it's overwritten on every build.

#![allow(unused_imports)]
#![allow(dead_code)]

use crate::__rt::*;

pub fn app() -> Primitive { let count: Signal<i32> = signal!(0_i32); let on_tap: Rc<dyn Fn()> = Rc::new(move || count.set(count.get() + 1)); let label = move || format!("Tapped {} times", count.get()); ui! { view(vec![ text("Hello, fiddle!").into(), text(label).into(), button("Tap me", move || on_tap()).into(), ]) } }
