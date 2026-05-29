//! `BuildElement` impls + tag aliases — the dispatch glue that
//! `ui! { Foo(...) }` / `jsx!` target for idea-ui components.
//!
//! `ui!` lowers a tag `Foo` to a plain struct literal plus a UFCS call:
//!
//! ```ignore
//! ::runtime_core::BuildElement::build(
//!     Foo { label: ("x").into(), ..<Foo as BuildElement>::defaults() }
//! )
//! ```
//!
//! so each component needs (1) an `impl BuildElement for FooProps` tying
//! its props struct to its function, and (2) a `Foo` tag that names that
//! props type. The `build_impl!` helper below stamps out both from one
//! line. This replaced the old per-component `#[macro_export]
//! macro_rules!`: dispatch resolves across crate boundaries by ordinary
//! path rules (no `#[macro_export]` / `#[macro_use]`), and the call site
//! is a real struct literal, so rust-analyzer gives field completion /
//! hover / go-to-def on every prop.
//!
//! `build_impl!` is a private DRY helper, NOT a per-component public
//! macro. The `&` form is for by-ref components (`fn foo(props:
//! &FooProps)`); the plain form is for by-value/container components
//! (`fn foo(props: FooProps)`) that consume their props to move children
//! out.
//!
//! Tags are emitted as `pub use FooProps as Foo` re-exports (not type
//! aliases) so the tag carries the props struct's own docs on hover, and
//! so `lib.rs` can surface them all with one `pub use invocations::*`.
//! `Btn` → `ButtonProps` because the `Button` tag is reserved for the
//! framework's `<Button>` primitive.
//!
//! `defaults()` is left as the trait's provided impl (`Self::default()`)
//! — every idea-ui `*Props` is `Default`, so omitting a prop takes its
//! default.

macro_rules! build_impl {
    // by-ref: `fn foo(props: &FooProps)`
    (& $func:path => $Props:path as $Tag:ident) => {
        pub use $Props as $Tag;
        #[automatically_derived]
        impl ::runtime_core::BuildElement for $Props {
            fn build(self) -> ::runtime_core::Element {
                $func(&self)
            }
        }
    };
    // by-value: `fn foo(props: FooProps)`
    ($func:path => $Props:path as $Tag:ident) => {
        pub use $Props as $Tag;
        #[automatically_derived]
        impl ::runtime_core::BuildElement for $Props {
            fn build(self) -> ::runtime_core::Element {
                $func(self)
            }
        }
    };
}

// ---- by-ref components ----
build_impl!(& crate::components::typography::typography => crate::components::typography::TypographyProps as Typography);
build_impl!(& crate::components::button::button         => crate::components::button::ButtonProps       as Btn);
build_impl!(& crate::components::field::field           => crate::components::field::FieldProps          as Field);
build_impl!(& crate::components::switch::switch         => crate::components::switch::SwitchProps        as Switch);
build_impl!(& crate::components::spinner::spinner       => crate::components::spinner::SpinnerProps      as Spinner);
build_impl!(& crate::components::divider::divider       => crate::components::divider::DividerProps      as Divider);
build_impl!(& crate::components::badge::badge           => crate::components::badge::BadgeProps          as Badge);
build_impl!(& crate::components::spacer::spacer         => crate::components::spacer::SpacerProps        as Spacer);
build_impl!(& crate::components::icon_button::icon_button => crate::components::icon_button::IconButtonProps as IconButton);
build_impl!(& crate::components::avatar::avatar         => crate::components::avatar::AvatarProps        as Avatar);
build_impl!(& crate::components::tag::tag               => crate::components::tag::TagProps              as Tag);
build_impl!(& crate::components::alert::alert           => crate::components::alert::AlertProps          as Alert);
build_impl!(& crate::components::skeleton::skeleton     => crate::components::skeleton::SkeletonProps    as Skeleton);

// ---- by-value / container components ----
build_impl!(crate::components::stack::stack     => crate::components::stack::StackProps     as Stack);
build_impl!(crate::components::card::card       => crate::components::card::CardProps       as Card);
build_impl!(crate::components::center::center   => crate::components::center::CenterProps   as Center);
build_impl!(crate::components::tabs::tabs       => crate::components::tabs::TabsProps       as Tabs);
build_impl!(crate::components::select::select   => crate::components::select::SelectProps   as Select);
build_impl!(crate::components::modal::modal     => crate::components::modal::ModalProps     as Modal);
build_impl!(crate::components::popover::popover => crate::components::popover::PopoverProps as Popover);
