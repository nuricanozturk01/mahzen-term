//! The Kubernetes info-card widget: context, namespace and pod health at a glance.

use mahzenterm_ui_kit::infocard::MAX_ROWS;
use mahzenterm_ui_kit::{InfoCard, InfoRow, InfoTone};

use crate::{Health, Pod};

/// Build the card for `context`/`namespace` (`prod`: the context looks like production) and a pod query result.
pub fn card(
    context: Option<&str>,
    namespace: &str,
    prod: bool,
    pods: &Result<Vec<Pod>, String>,
) -> InfoCard {
    let mut card = InfoCard {
        title: "Kubernetes".into(),
        ..InfoCard::default()
    };
    let Some(context) = context else {
        card.badge = "no context".into();
        card.empty = "kubectl has no current context".into();
        return card;
    };
    card.badge = if prod { "PROD".into() } else { String::new() };
    card.rows.push(InfoRow::new(
        "context",
        context,
        if prod {
            InfoTone::Bad
        } else {
            InfoTone::Accent
        },
    ));
    card.rows
        .push(InfoRow::new("namespace", namespace, InfoTone::Normal));
    let pods = match pods {
        Ok(p) => p,
        Err(e) => {
            card.rows
                .push(InfoRow::new("pods", e.clone(), InfoTone::Bad));
            return card;
        }
    };
    let healthy = pods.iter().filter(|p| p.health() == Health::Good).count();
    let tone = if healthy == pods.len() {
        InfoTone::Good
    } else {
        InfoTone::Warn
    };
    card.rows.push(InfoRow::new(
        "pods",
        format!("{healthy}/{} healthy", pods.len()),
        tone,
    ));
    let mut unhealthy: Vec<&Pod> = pods.iter().filter(|p| p.health() != Health::Good).collect();
    unhealthy.sort_by_key(|p| (p.health() == Health::Pending, std::cmp::Reverse(p.restarts)));
    let room = MAX_ROWS - card.rows.len();
    for p in unhealthy.iter().take(if unhealthy.len() > room {
        room - 1
    } else {
        room
    }) {
        let tone = if p.health() == Health::Bad {
            InfoTone::Bad
        } else {
            InfoTone::Warn
        };
        let restarts = if p.restarts > 0 {
            format!(" \u{21bb}{}", p.restarts)
        } else {
            String::new()
        };
        card.rows.push(InfoRow::new(
            p.name.clone(),
            format!("{}{restarts}", p.status),
            tone,
        ));
    }
    if unhealthy.len() > room {
        card.rows.push(InfoRow::new(
            format!("+{} more", unhealthy.len() - (room - 1)),
            "",
            InfoTone::Dim,
        ));
    }
    card
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pod(name: &str, status: &str, ready: u32, restarts: u32) -> Pod {
        Pod {
            name: name.into(),
            namespace: "default".into(),
            status: status.into(),
            ready,
            total: 1,
            restarts,
            age_secs: None,
        }
    }

    #[test]
    fn unhealthy_pods_are_listed_after_the_summary() {
        let pods = vec![
            pod("a", "Running", 1, 0),
            pod("b", "CrashLoopBackOff", 0, 7),
            pod("c", "Pending", 0, 0),
        ];
        let c = card(Some("dev"), "default", false, &Ok(pods));
        assert_eq!(c.rows[2].value, "1/3 healthy");
        assert_eq!(c.rows[3].label, "b");
        assert_eq!(c.rows[3].tone, InfoTone::Bad);
        assert!(c.rows[3].value.contains("\u{21bb}7"));
        assert_eq!(c.rows[4].label, "c");
    }

    #[test]
    fn production_is_marked_and_errors_are_shown() {
        let c = card(
            Some("prod-eu"),
            "x",
            true,
            &Err("cluster unreachable".into()),
        );
        assert_eq!(c.badge, "PROD");
        assert_eq!(c.rows[0].tone, InfoTone::Bad);
        assert_eq!(c.rows.last().unwrap().value, "cluster unreachable");
        assert!(!card(None, "x", false, &Ok(Vec::new())).empty.is_empty());
    }

    #[test]
    fn a_crowded_namespace_is_summarised() {
        let pods: Vec<Pod> = (0..40)
            .map(|i| pod(&format!("p{i}"), "Error", 0, 0))
            .collect();
        let c = card(Some("c"), "n", false, &Ok(pods));
        assert_eq!(c.rows.len(), MAX_ROWS);
        assert!(c.rows.last().unwrap().label.starts_with('+'));
    }
}
