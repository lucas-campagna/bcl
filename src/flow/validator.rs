use crate::flow::{Action, Condition};

pub fn validate_action(
    action: &Action,
    flow_name: &str,
    action_index: usize,
    warnings: &mut Vec<String>,
) {
    if let Action::Call { flow, .. } = action {
        warnings.push(format!(
            "flow '{}' action {} calls '{}' — target existence not checked until runtime",
            flow_name, action_index, flow
        ));
    }

    if let Action::If { condition, .. } = action {
        validate_condition(condition, flow_name, action_index, warnings);
    }

    let _ = (flow_name, action_index);
}

fn validate_condition(
    cond: &Condition,
    flow_name: &str,
    action_index: usize,
    warnings: &mut Vec<String>,
) {
    match cond {
        Condition::Not(inner) => {
            validate_condition(inner, flow_name, action_index, warnings);
        }
        Condition::And(conds) | Condition::Or(conds) => {
            for c in conds {
                validate_condition(c, flow_name, action_index, warnings);
            }
        }
        _ => {}
    }
}
