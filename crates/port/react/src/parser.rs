//! TSX → porter IR for React. The actual parsing is shared via
//! `port-tsx`; this file plugs in the React-specific reactive-
//! primitive recognition (`useState`/`useEffect`/…).

use port_core::ir::{Module, PortReport};
use port_core::{ParseError, Parser};
use port_tsx::{CallContext, LiftedCall, Lifter, ReadStyle};

use crate::hooks::{builtin, HookClass, HookRegistry, Mechanical};

pub struct ReactParser {
    hooks: HookRegistry,
}

impl ReactParser {
    pub fn new() -> Self {
        Self { hooks: builtin() }
    }
}

impl Default for ReactParser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser for ReactParser {
    fn parse(&self, source: &str) -> Result<(Module, PortReport), ParseError> {
        port_tsx::parse_and_lift(source, "port-react", self)
    }
}

impl Lifter for ReactParser {
    fn classify_call(&self, callee: &str, ctx: &CallContext) -> Option<LiftedCall> {
        match self.hooks.classify(callee) {
            HookClass::Mechanical(Mechanical::UseState) => {
                // useState only makes sense in a let binding.
                ctx.binding.as_ref()?;
                Some(LiftedCall::State)
            }
            HookClass::Mechanical(Mechanical::UseEffect)
            | HookClass::Mechanical(Mechanical::UseLayoutEffect) => {
                Some(LiftedCall::Effect { has_deps: true })
            }
            // useMemo / useCallback / useRef: idealyst has no
            // direct equivalent to memo/callback (signals
            // auto-track, closures capture). For now route to a
            // plain function call so the binding + arguments are
            // preserved verbatim; the AI pass (or a future
            // framework primitive) can unwrap them.
            HookClass::Mechanical(Mechanical::UseMemo)
            | HookClass::Mechanical(Mechanical::UseCallback)
            | HookClass::Mechanical(Mechanical::UseRef) => None,
            HookClass::Mechanical(Mechanical::UseContext) => Some(LiftedCall::Inject),
            HookClass::Unknown => None,
        }
    }

    fn signal_read_style(&self) -> ReadStyle {
        // React: bare-ident read (`{count}`).
        ReadStyle::BareIdent
    }
}

// Back-compat: older callers used `StubParser::new()`.
pub type StubParser = ReactParser;
