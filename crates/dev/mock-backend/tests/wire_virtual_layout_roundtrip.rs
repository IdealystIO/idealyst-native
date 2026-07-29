//! `WireVirtualLayout` reconciliation pin: the wire mirror and the ONE
//! in-memory definition (`runtime_shared::primitives::virtualizer::
//! VirtualLayout`, re-exported through `runtime_core`) must stay
//! bijective through the pair of conversions —
//! `dev_server::convert_out::virtual_layout_to_wire` (recording side)
//! and `dev_client::convert::wire_virtual_layout` (replay side).
//!
//! Both conversions are written as exhaustive matches, so a NEW
//! `Axis`/`Lanes`/`WireLanes` variant breaks compile at the conversion
//! sites; this test additionally pins that every EXISTING variant
//! survives core → wire → core (and the wire serde encoding) unchanged.

use runtime_core::primitives::virtualizer::{Axis, Lanes, VirtualLayout};

fn roundtrip(l: VirtualLayout) -> VirtualLayout {
    let wire_form = dev_server::convert_out::virtual_layout_to_wire(l);
    // Through the real codec too: the wire mirror's serde shape is part
    // of the protocol (all fields `#[serde(default)]`-tolerant).
    let bytes = wire::codec::encode(&wire_form).expect("encode WireVirtualLayout");
    let decoded: wire::WireVirtualLayout = wire::codec::decode(&bytes).expect("decode");
    dev_client::convert::wire_virtual_layout(decoded)
}

#[test]
fn virtual_layout_roundtrips_all_variants() {
    let cases = [
        VirtualLayout::default(),
        VirtualLayout { axis: Axis::Horizontal, ..Default::default() },
        VirtualLayout {
            axis: Axis::Vertical,
            lanes: Lanes::Fixed(3),
            main_spacing: 8.0,
            cross_spacing: 12.0,
        },
        VirtualLayout {
            axis: Axis::Horizontal,
            lanes: Lanes::AutoFit { min_cross: 140.0 },
            main_spacing: 4.0,
            cross_spacing: 4.0,
        },
    ];
    for case in cases {
        assert_eq!(
            roundtrip(case),
            case,
            "VirtualLayout must survive core → wire → core unchanged"
        );
    }
}

#[test]
fn empty_wire_layout_decodes_as_default_list() {
    // An older peer that omits every field must decode as the default
    // single-lane vertical list — the `#[serde(default)]` tolerance the
    // wire type documents.
    let decoded: wire::WireVirtualLayout =
        wire::codec::decode(b"{}").expect("all-default decode");
    assert_eq!(
        dev_client::convert::wire_virtual_layout(decoded),
        VirtualLayout::default(),
    );
}
