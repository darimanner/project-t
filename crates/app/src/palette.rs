//! Command palette state (Cmd+K): query + selection over the host list.
//! Rendering and key routing live in `workspace.rs`.

use core_store::Host;

use crate::text_input::InputState;

#[derive(Default)]
pub struct Palette {
    pub input: InputState,
    /// Index into the *filtered* list.
    pub selected: usize,
}

impl Palette {
    /// Clamp selection after the filter results change.
    pub fn clamp_selection(&mut self, filtered_len: usize) {
        if filtered_len == 0 {
            self.selected = 0;
        } else if self.selected >= filtered_len {
            self.selected = filtered_len - 1;
        }
    }

    pub fn move_selection(&mut self, delta: i32, filtered_len: usize) {
        if filtered_len == 0 {
            return;
        }
        let len = filtered_len as i32;
        self.selected = ((self.selected as i32 + delta).rem_euclid(len)) as usize;
    }
}

/// Indices of hosts whose label/host/user contains `query`, case-insensitive.
/// An empty query matches everything.
pub fn filter_hosts(hosts: &[Host], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    hosts
        .iter()
        .enumerate()
        .filter(|(_, h)| {
            q.is_empty()
                || h.label.to_lowercase().contains(&q)
                || h.host.to_lowercase().contains(&q)
                || h.username.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_store::AuthKind;

    fn host(label: &str, addr: &str, user: &str) -> Host {
        Host {
            id: 0,
            label: label.to_owned(),
            host: addr.to_owned(),
            port: 22,
            username: user.to_owned(),
            auth: AuthKind::Password,
            key_path: None,
        }
    }

    #[test]
    fn empty_query_matches_all() {
        let hosts = vec![
            host("prod", "1.2.3.4", "root"),
            host("stg", "5.6.7.8", "app"),
        ];
        assert_eq!(filter_hosts(&hosts, ""), vec![0, 1]);
    }

    #[test]
    fn matches_label_host_and_user_case_insensitive() {
        let hosts = vec![
            host("Prod", "10.0.0.1", "root"),
            host("stg", "5.6.7.8", "deploy"),
        ];
        assert_eq!(filter_hosts(&hosts, "PROD"), vec![0]);
        assert_eq!(filter_hosts(&hosts, "5.6"), vec![1]);
        assert_eq!(filter_hosts(&hosts, "deploy"), vec![1]);
        assert!(filter_hosts(&hosts, "nothing").is_empty());
    }

    #[test]
    fn selection_wraps_and_clamps() {
        let mut p = Palette::default();
        p.move_selection(-1, 3);
        assert_eq!(p.selected, 2); // wraps backwards
        p.move_selection(1, 3);
        assert_eq!(p.selected, 0);
        p.selected = 5;
        p.clamp_selection(2);
        assert_eq!(p.selected, 1);
        p.clamp_selection(0);
        assert_eq!(p.selected, 0);
    }
}
