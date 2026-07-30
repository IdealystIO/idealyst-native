use runtime_core::{stylesheet, FlexDirection};

stylesheet! {
    pub Panel<()> {
        base(_t) {                       // Xs — the mobile-first base
            flex_direction: FlexDirection::Column,
            padding: 12.0,
        }
        breakpoint md(_t) {              // >= 768 dp
            flex_direction: FlexDirection::Row,
            padding: 20.0,
        }
        breakpoint lg(_t) { padding: 32.0 }   // >= 1024 dp
    }
}
