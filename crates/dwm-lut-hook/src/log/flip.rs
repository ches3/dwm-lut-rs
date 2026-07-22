#![cfg_attr(not(debug_assertions), allow(dead_code))]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IndependentFlipRejectReason {
    PageNotWritable,
    UnexpectedValue(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IndependentFlipOutcome {
    Applied,
    Restored,
    Rejected(IndependentFlipRejectReason),
}

pub(crate) fn independent_flip(outcome: IndependentFlipOutcome) {
    #[cfg(debug_assertions)]
    {
        match outcome {
            IndependentFlipOutcome::Applied => {
                super::write(format_args!(
                    "event=independent_flip outcome=applied value=1"
                ));
            }
            IndependentFlipOutcome::Restored => {
                super::write(format_args!("event=independent_flip outcome=restored"));
            }
            IndependentFlipOutcome::Rejected(IndependentFlipRejectReason::PageNotWritable) => {
                super::write(format_args!(
                    "event=independent_flip outcome=rejected reason={}",
                    super::quoted("page_not_writable")
                ));
            }
            IndependentFlipOutcome::Rejected(IndependentFlipRejectReason::UnexpectedValue(
                value,
            )) => {
                super::write(format_args!(
                    "event=independent_flip outcome=rejected reason={} value={value}",
                    super::quoted("unexpected_value")
                ));
            }
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = outcome;
}

pub(crate) fn overlays_enabled_override(value: Option<bool>) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=overlays_enabled_override value={value:?}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = value;
}

pub(crate) fn flip_gate_denied(gate: &str, denied_total: u64) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=flip_gate_denied gate={gate} denied_total={denied_total}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (gate, denied_total);
}
