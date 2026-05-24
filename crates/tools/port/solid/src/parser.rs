//! Solid TSX → porter IR. Parsing is shared with port-react via
//! `port-tsx`; this file plugs in the Solid-specific primitive
//! recognition.

use port_core::ir::{Module, PortReport};
use port_core::{ParseError, Parser};
use port_tsx::{CallContext, LiftedCall, Lifter, ReadStyle};

use crate::primitives::{builtin, Mechanical, PrimitiveClass, PrimitiveRegistry};

pub struct SolidParser {
    primitives: PrimitiveRegistry,
}

impl SolidParser {
    pub fn new() -> Self {
        Self { primitives: builtin() }
    }
}

impl Default for SolidParser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser for SolidParser {
    fn parse(&self, source: &str) -> Result<(Module, PortReport), ParseError> {
        port_tsx::parse_and_lift(source, "port-solid", self)
    }
}

impl Lifter for SolidParser {
    fn classify_call(&self, callee: &str, ctx: &CallContext) -> Option<LiftedCall> {
        match self.primitives.classify(callee) {
            PrimitiveClass::Mechanical(Mechanical::CreateSignal) => {
                ctx.binding.as_ref()?;
                Some(LiftedCall::State)
            }
            PrimitiveClass::Mechanical(Mechanical::CreateEffect)
            | PrimitiveClass::Mechanical(Mechanical::OnMount) => {
                // Solid effects auto-track — no deps array.
                Some(LiftedCall::Effect { has_deps: false })
            }
            PrimitiveClass::Mechanical(Mechanical::CreateMemo)
            | PrimitiveClass::Mechanical(Mechanical::CreateResource)
            | PrimitiveClass::Mechanical(Mechanical::OnCleanup) => Some(LiftedCall::Drop),
            PrimitiveClass::Mechanical(Mechanical::UseContext) => Some(LiftedCall::Inject),
            PrimitiveClass::Unknown => None,
        }
    }

    fn signal_read_style(&self) -> ReadStyle {
        // Solid: `count()` reads. Lifter rewrites to `count.get()`.
        ReadStyle::CallExpression
    }
}

pub type StubParser = SolidParser;
