#![cfg_attr(not(debug_assertions), allow(dead_code))]

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
