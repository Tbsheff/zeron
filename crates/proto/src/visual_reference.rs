pub const REFERENCE: &str = include_str!("visual_reference.md");

pub const SKILL_NAME: &str = "visuals";

pub fn topics() -> Vec<&'static str> {
    sections().map(|(topic, _)| topic).collect()
}

pub fn section(topic: &str) -> Option<&'static str> {
    let wanted = topic.trim().to_ascii_lowercase();
    sections()
        .find(|(name, _)| *name == wanted)
        .map(|(_, body)| body)
}

fn sections() -> impl Iterator<Item = (&'static str, &'static str)> {
    REFERENCE.split("## ").filter_map(|chunk| {
        let (topic, body) = chunk.split_once('\n')?;
        let topic = topic.trim();
        (!topic.is_empty()).then_some((topic, body.trim()))
    })
}

pub fn check_error_with_hint(message: &str) -> String {
    let topic = topic_for_error(message);
    format!(
        "{message} Fix the line and call show_visual again. Props for this: visual_reference {{\"topic\":\"{topic}\"}} (or the zeron:visuals skill)."
    )
}

pub fn topic_for_error(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    prop_topic(&lower)
        .or_else(|| node_topic(&lower))
        .unwrap_or_else(|| keyword_topic(&lower))
}

fn prop_topic(lower: &str) -> Option<&'static str> {
    lower
        .split('\'')
        .filter_map(|quoted| match quoted {
            "width" | "grow" | "key" | "gap" | "align" | "justify" | "wrap" => Some("layout"),
            "fill" | "bg" | "background" | "border" | "shadow" | "padding" | "radius" | "blur"
            | "opacity" | "font" | "family" | "font_family" | "fontfamily" | "font_size"
            | "fontsize" => Some("style"),
            "color" => Some("colors"),
            _ => None,
        })
        .last()
}

fn node_topic(lower: &str) -> Option<&'static str> {
    let at = lower.rfind("ui.")? + 3;
    let kind: String = lower[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    Some(match kind.as_str() {
        "column" | "row" | "grid" | "rule" | "space" | "link" => "layout",
        "text" | "span" => "text",
        "stat" | "table" | "chart" => "data",
        "tabs" | "toggle" | "slider" | "button" | "answer" => "controls",
        "canvas" => "canvas",
        "scene" => "scene",
        "shader" => "shader",
        _ => return None,
    })
}

fn has_method(lower: &str, receiver: &str) -> bool {
    lower.match_indices(receiver).any(|(at, _)| {
        lower[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
    })
}

fn keyword_topic(lower: &str) -> &'static str {
    let has = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    if has(&["glsl", "shader", "ichannel", "mainimage"]) {
        "shader"
    } else if has(&["asset", ".glb", ".hdr", "texture", "environment", "sprite", "model"]) {
        "assets"
    } else if has(&["scene"]) || has_method(lower, "s:") {
        "scene"
    } else if has(&["canvas"]) || has_method(lower, "g:") {
        "canvas"
    } else if has(&["color", "token", "hex"]) {
        "colors"
    } else if has(&["fill", "border", "shadow", "padding", "font", "background"]) {
        "style"
    } else if has(&["chart", "series", "table", "stat"]) {
        "data"
    } else if has(&["tabs", "toggle", "slider", "button", "on_change", "on_click"]) {
        "controls"
    } else if has(&["text", "span", "size", "weight"]) {
        "text"
    } else if has(&["column", "row", "grid", "gap", "align", "justify"]) {
        "layout"
    } else {
        "basics"
    }
}

pub fn skill_markdown() -> String {
    format!(
        "---\nname: {SKILL_NAME}\ndescription: Full API for Zeron's show_visual tool (Luau visual scripts: layout, text, stats, tables, charts, controls, 2D canvas, 3D scene, assets, GLSL shaders, colors, style rules). Use before writing or fixing a show_visual script.\n---\n\n# Zeron visuals\n\nCall the Zeron tool show_visual with {{ title, script, assets? }} before your final reply.\n\n{REFERENCE}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_has_a_body_and_errors_map_to_real_topics() {
        let topics = topics();
        assert!(topics.contains(&"basics") && topics.contains(&"style") && topics.contains(&"shader"));
        for topic in &topics {
            assert!(section(topic).is_some_and(|body| body.len() > 40), "{topic}");
        }
        for message in ["GLSL error in pass 1", "unknown prop fill", "s:box needs at", "bad color 'red'", "x"] {
            assert!(topics.contains(&topic_for_error(message)), "{message}");
        }
    }

    #[test]
    fn errors_map_to_the_topic_that_documents_the_prop_or_node() {
        let cases = [
            ("ui.text 'grow' must be true or false, not a number", "layout"),
            ("after switching 'Runs': ui.text has no 'grow'", "layout"),
            ("ui.row has no 'padding': visuals have no containers", "style"),
            ("ui.chart needs series = { { name = ..., values = { ... } } }", "data"),
            ("ui.table has no 'width_px' (it takes columns, rows, hover)", "data"),
            ("ui.answer takes a string, not a nil", "controls"),
            ("ui.text 'weight' must be a number or a name", "text"),
            ("in a scene draw: s:box needs at", "scene"),
            ("options: bad value", "basics"),
        ];
        for (message, topic) in cases {
            assert_eq!(topic_for_error(message), topic, "{message}");
        }
    }

    #[test]
    fn the_skill_has_front_matter_and_the_whole_reference() {
        let skill = skill_markdown();
        assert!(skill.starts_with("---\nname: visuals\ndescription: "));
        assert!(skill.ends_with(REFERENCE));
    }
}
