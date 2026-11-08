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

pub fn topic_for_error(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    if has(&["glsl", "shader", "ichannel", "mainimage"]) {
        "shader"
    } else if has(&["asset", ".glb", ".hdr", "texture", "environment", "sprite", "model"]) {
        "assets"
    } else if has(&["scene", "s:"]) {
        "scene"
    } else if has(&["canvas", "g:"]) {
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
    fn the_skill_has_front_matter_and_the_whole_reference() {
        let skill = skill_markdown();
        assert!(skill.starts_with("---\nname: visuals\ndescription: "));
        assert!(skill.ends_with(REFERENCE));
    }
}
