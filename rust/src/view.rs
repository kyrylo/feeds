use chrono::Datelike;
use minijinja::{AutoEscape, Environment};
use serde_json::{Value, json};
use std::sync::OnceLock;

use crate::assets;

fn environment() -> &'static Environment<'static> {
    static ENV: OnceLock<Environment<'static>> = OnceLock::new();
    ENV.get_or_init(|| {
        let mut env = Environment::new();
        env.set_auto_escape_callback(|_| AutoEscape::Html);
        for (name, source) in [
            (
                "simple-layout.html",
                include_str!("../templates/simple-layout.html"),
            ),
            ("activity.html", include_str!("../templates/activity.html")),
            (
                "activity-tab-form.html",
                include_str!("../templates/activity-tab-form.html"),
            ),
            (
                "activity-rows.html",
                include_str!("../templates/activity-rows.html"),
            ),
            (
                "activity-detail.html",
                include_str!("../templates/activity-detail.html"),
            ),
            (
                "activity-event.html",
                include_str!("../templates/activity-event.html"),
            ),
            (
                "activity-history.html",
                include_str!("../templates/activity-history.html"),
            ),
            ("connect.html", include_str!("../templates/connect.html")),
        ] {
            env.add_template(name, source).unwrap();
        }
        env.set_formatter(|out, state, value| {
            if value.is_none() {
                Ok(())
            } else {
                minijinja::escape_formatter(out, state, value)
            }
        });
        env.add_template("layout.html", include_str!("../templates/layout.html"))
            .unwrap();
        env.add_template(
            "public-sidebar.html",
            include_str!("../templates/public-sidebar.html"),
        )
        .unwrap();
        env.add_template("auth.html", include_str!("../templates/auth.html"))
            .unwrap();
        env.add_template("password.html", include_str!("../templates/password.html"))
            .unwrap();
        env.add_template("account.html", include_str!("../templates/account.html"))
            .unwrap();
        env.add_template(
            "settings-sidebar.html",
            include_str!("../templates/settings-sidebar.html"),
        )
        .unwrap();
        env.add_template("settings.html", include_str!("../templates/settings.html"))
            .unwrap();
        env.add_template(
            "sinks-sidebar.html",
            include_str!("../templates/sinks-sidebar.html"),
        )
        .unwrap();
        env.add_template(
            "sidebar-content.html",
            include_str!("../templates/sidebar-content.html"),
        )
        .unwrap();
        env.add_template("icons.html", include_str!("../templates/icons.html"))
            .unwrap();
        env.add_template("welcome.html", include_str!("../templates/welcome.html"))
            .unwrap();
        env.add_template(
            "folder-form.html",
            include_str!("../templates/folder-form.html"),
        )
        .unwrap();
        env.add_template(
            "event-types.html",
            include_str!("../templates/event-types.html"),
        )
        .unwrap();
        env.add_template(
            "event-type.html",
            include_str!("../templates/event-type.html"),
        )
        .unwrap();
        env.add_template(
            "event-types-stream.html",
            include_str!("../templates/event-types-stream.html"),
        )
        .unwrap();
        env.add_template(
            "sink-form.html",
            include_str!("../templates/sink-form.html"),
        )
        .unwrap();
        env.add_template("feed-row.html", include_str!("../templates/feed-row.html"))
            .unwrap();
        env.add_template(
            "event-row-content.html",
            include_str!("../templates/event-row-content.html"),
        )
        .unwrap();
        env.add_template(
            "older-loader.html",
            include_str!("../templates/older-loader.html"),
        )
        .unwrap();
        env.add_template(
            "older-stream.html",
            include_str!("../templates/older-stream.html"),
        )
        .unwrap();
        env.add_template(
            "sidebar-stream.html",
            include_str!("../templates/sidebar-stream.html"),
        )
        .unwrap();
        env.add_template(
            "created-stream.html",
            include_str!("../templates/created-stream.html"),
        )
        .unwrap();
        env.add_template("column.html", include_str!("../templates/column.html"))
            .unwrap();
        env.add_template(
            "column-batch.html",
            include_str!("../templates/column-batch.html"),
        )
        .unwrap();
        env.add_template(
            "column-more.html",
            include_str!("../templates/column-more.html"),
        )
        .unwrap();
        env.add_template(
            "column-stream.html",
            include_str!("../templates/column-stream.html"),
        )
        .unwrap();
        env.add_template(
            "column-edit.html",
            include_str!("../templates/column-edit.html"),
        )
        .unwrap();
        env.add_template(
            "feed-types.html",
            include_str!("../templates/feed-types.html"),
        )
        .unwrap();
        env.add_template(
            "live-feed.html",
            include_str!("../templates/live-feed.html"),
        )
        .unwrap();
        env.add_template("feed.html", include_str!("../templates/feed.html"))
            .unwrap();
        env.add_template(
            "feed-tools.html",
            include_str!("../templates/feed-tools.html"),
        )
        .unwrap();
        env.add_template(
            "feed-status.html",
            include_str!("../templates/feed-status.html"),
        )
        .unwrap();
        env.add_template("event.html", include_str!("../templates/event.html"))
            .unwrap();
        env.add_template(
            "event-detail.html",
            include_str!("../templates/event-detail.html"),
        )
        .unwrap();
        env
    })
}

pub fn render(page: &str, mut context: Value) -> Result<String, minijinja::Error> {
    context["page"] = json!(page);
    if context.get("sidebar").is_none() {
        context["sidebar"] = json!("public-sidebar.html");
    }
    context["year"] = json!(chrono::Utc::now().year());
    context["styles"] = json!(assets::STYLES);
    context["importmap"] = json!(assets::IMPORTMAP);
    context["activity_style"] = json!(assets::ACTIVITY_STYLE);
    context["activity_script"] = json!(assets::ACTIVITY_SCRIPT);
    let legacy_page = !page.starts_with("activity")
        && !matches!(
            page,
            "connect.html" | "auth.html" | "password.html" | "settings.html"
        );
    let layout = if context["demo"] == true || legacy_page {
        "layout.html"
    } else {
        "simple-layout.html"
    };
    environment()
        .get_template(layout)?
        .render(template_value(context))
}

pub fn fragment(name: &str, context: Value) -> Result<String, minijinja::Error> {
    environment()
        .get_template(name)?
        .render(template_value(context))
}

// serde_json's arbitrary_precision feature serializes numbers through a private
// map representation. Crossing through Serialize directly leaks that map into
// templates and breaks numeric comparisons such as user roles. Convert the
// value tree explicitly; event JSON itself keeps its original numeric precision.
fn template_value(value: Value) -> minijinja::Value {
    match value {
        Value::Null => minijinja::Value::from(()),
        Value::Bool(value) => minijinja::Value::from(value),
        Value::String(value) => minijinja::Value::from(value),
        Value::Number(value) => {
            if let Some(integer) = value.as_i64() {
                minijinja::Value::from(integer)
            } else if let Some(integer) = value.as_u64() {
                minijinja::Value::from(integer)
            } else {
                minijinja::Value::from(value.to_string())
            }
        }
        Value::Array(values) => {
            minijinja::Value::from(values.into_iter().map(template_value).collect::<Vec<_>>())
        }
        Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| (key, template_value(value)))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_roles_and_escaped_form_values_render_as_browser_values() {
        let html=render("settings.html",json!({"title":"change nickname","mode":"nickname","sidebar":"settings-sidebar.html",
            "current_user":{"id":42,"role":2,"nickname":"<script>unsafe</script>"},"csrf":"synthetic","action":"/settings/nickname"})).unwrap();
        assert!(!html.contains("$serde_json"));
        assert!(!html.contains("/settings/members"));
        assert!(!html.contains("/settings/invitations"));
        assert!(html.contains("&lt;script&gt;unsafe&lt;&#x2f;script&gt;"));
        assert!(!html.contains("<script>unsafe"));
        let public = render(
            "auth.html",
            json!({"title":"sign in","mode":"login","email_address":null,"action":"/session"}),
        )
        .unwrap();
        assert!(
            public
                .replace("&#x2f;", "/")
                .contains("action=\"/session\"")
        );
        assert!(!public.contains("None"));
        assert!(!public.contains("$serde_json"));
    }
}
