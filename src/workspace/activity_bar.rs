use super::View;
use serde_json::{Value, json};

/// The setting's value as the stored list: unknown and duplicated ids dropped, Explorer ensured.
pub fn parse(value: &Value) -> (Vec<View>, Vec<String>) {
    let default = || View::ALL.to_vec();
    let Some(ids) = value.as_array() else {
        return (
            default(),
            vec![
                "antler.activityBar.views: expected an array of strings; using the default".into(),
            ],
        );
    };
    if ids.is_empty() || ids.iter().any(|id| !id.is_string()) {
        return (default(), vec!["antler.activityBar.views: expected a non-empty array of strings; using the default".into()]);
    }
    let mut list = Vec::new();
    let mut warnings = Vec::new();
    for id in ids.iter().filter_map(Value::as_str) {
        match View::from_id(id) {
            None => warnings.push(format!(
                "antler.activityBar.views: unknown view \"{id}\" ignored"
            )),
            Some(view) if list.contains(&view) => warnings.push(format!(
                "antler.activityBar.views: duplicate view \"{id}\" ignored"
            )),
            Some(view) => list.push(view),
        }
    }
    if !list.contains(&View::Explorer) {
        list.insert(0, View::Explorer);
    }
    (list, warnings)
}

/// The icons to draw, in order: Ports only remotely; append a hidden active view.
pub fn shown(list: &[View], remote: bool, active: Option<View>) -> Vec<View> {
    let mut shown: Vec<_> = list
        .iter()
        .copied()
        .filter(|view| remote || *view != View::Ports)
        .collect();
    if let Some(active) = active.filter(|view| remote || *view != View::Ports)
        && !shown.contains(&active)
    {
        shown.push(active);
    }
    shown
}

/// `view` moved to `onto`'s position in `list`; same list if `view == onto` or either is missing.
pub fn move_to(list: &[View], view: View, onto: View) -> Vec<View> {
    let (Some(from), Some(target_index)) = (list.iter().position(|v| *v == view), list.iter().position(|v| *v == onto)) else {
        return list.to_vec();
    };
    if from == target_index {
        return list.to_vec();
    }
    let mut moved = list.to_vec();
    moved.remove(from);
    moved.insert(target_index, view);
    moved
}

/// The setting's value for `list`.
pub fn to_json(list: &[View]) -> Value {
    json!(list.iter().map(|view| view.id()).collect::<Vec<_>>())
}

/// `list` without `view`; Explorer is never removed.
pub fn hide(list: &[View], view: View) -> Vec<View> {
    if view == View::Explorer {
        return list.to_vec();
    }
    list.iter().copied().filter(|item| *item != view).collect()
}

/// `list` with `view` appended at the end; unchanged if already there.
pub fn show(list: &[View], view: View) -> Vec<View> {
    let mut list = list.to_vec();
    if !list.contains(&view) {
        list.push(view);
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::View;
    use serde_json::json;

    #[test]
    fn defaults_to_all_views() {
        assert_eq!(
            parse(&json!(View::ALL.map(View::id))),
            (View::ALL.to_vec(), vec![])
        );
    }

    #[test]
    fn explorer_is_inserted_first_or_kept_in_place() {
        assert_eq!(parse(&json!(["git"])).0, vec![View::Explorer, View::Git]);
        assert_eq!(
            parse(&json!(["git", "explorer"])).0,
            vec![View::Git, View::Explorer]
        );
    }

    #[test]
    fn unknown_and_duplicate_ids_are_ignored_with_warnings() {
        let (list, warnings) = parse(&json!(["git", "wat", "git"]));
        assert_eq!(list, vec![View::Explorer, View::Git]);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("wat"));
        assert!(warnings[1].contains("git"));
    }

    #[test]
    fn empty_and_non_array_values_use_default_with_warning() {
        for value in [json!([]), json!("git"), json!(["git", 42])] {
            let (list, warnings) = parse(&value);
            assert_eq!(list, View::ALL);
            assert_eq!(warnings.len(), 1);
        }
    }

    #[test]
    fn ports_are_only_shown_remotely() {
        let list = View::ALL.to_vec();
        assert!(!shown(&list, false, None).contains(&View::Ports));
        assert!(shown(&list, true, None).contains(&View::Ports));
    }

    #[test]
    fn hidden_active_view_is_appended_until_another_view_is_active() {
        let list = vec![View::Explorer, View::Git];
        assert_eq!(
            shown(&list, false, Some(View::Search)),
            vec![View::Explorer, View::Git, View::Search]
        );
        assert_eq!(shown(&list, false, Some(View::Git)), list);
    }

    #[test]
    fn setting_round_trips_view_ids() {
        let list = vec![View::Explorer, View::Projects];
        assert_eq!(to_json(&list), json!(["explorer", "projects"]));
    }

    #[test]
    fn moves_views_to_the_target_position() {
        let list = View::ALL.to_vec();
        assert_eq!(move_to(&list, View::Projects, View::Explorer), vec![View::Projects, View::Explorer, View::Git, View::Search, View::Ports]);
        assert_eq!(move_to(&list, View::Git, View::Ports), vec![View::Explorer, View::Search, View::Projects, View::Ports, View::Git]);
    }

    #[test]
    fn moving_onto_itself_changes_nothing() {
        let list = View::ALL.to_vec();
        assert_eq!(move_to(&list, View::Git, View::Git), list);
    }

    #[test]
    fn reordering_in_local_window_keeps_hidden_ports_in_the_list() {
        let list = View::ALL.to_vec();
        assert_eq!(shown(&move_to(&list, View::Projects, View::Explorer), false, None), vec![View::Projects, View::Explorer, View::Git, View::Search]);
    }

    #[test]
    fn hide_removes_view_but_keeps_explorer() {
        let list = View::ALL.to_vec();
        assert_eq!(
            hide(&list, View::Git),
            vec![View::Explorer, View::Search, View::Projects, View::Ports]
        );
        assert_eq!(hide(&list, View::Explorer), list);
    }

    #[test]
    fn show_appends_view_only_when_hidden() {
        let list = vec![View::Explorer, View::Git];
        assert_eq!(
            show(&list, View::Search),
            vec![View::Explorer, View::Git, View::Search]
        );
        assert_eq!(show(&list, View::Git), list);
    }
}
