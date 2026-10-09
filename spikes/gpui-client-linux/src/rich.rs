use std::{path::PathBuf, sync::Arc};

use gpui::{
    App, Context, ImageSource, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    Render, Resource, SharedString, SharedUri, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, div, img, prelude::FluentBuilder as _,
};
use gpui_base::text::{
    MarkdownNode, MarkdownParseContext, MarkdownPlugin, TextView, markdown_ast,
};
use gpui_component::{h_flex, v_flex};

use crate::support::{Assets, Palette, note, u};

pub struct MentionPlugin;

impl MarkdownPlugin for MentionPlugin {
    fn name(&self) -> &str {
        "mention"
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        _cx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::Link(link) = node else {
            return None;
        };
        let name = link.url.strip_prefix("mention:")?;
        Some(MarkdownNode::new("mention", name.to_string()).text(format!("@{name}")))
    }

    fn render(&self, node: &MarkdownNode, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .px(u(4.0))
            .rounded(u(4.0))
            .bg(Palette::terracotta().opacity(0.18))
            .text_color(Palette::terracotta())
            .child(SharedString::from(node.as_text().to_string()))
    }
}

pub fn resolve_image(uri: &SharedUri) -> ImageSource {
    match uri.strip_prefix("file://") {
        Some(path) => ImageSource::Resource(Resource::Path(Arc::from(PathBuf::from(path)))),
        None => ImageSource::Resource(Resource::Uri(uri.clone())),
    }
}

pub fn message_view(id: impl Into<gpui::ElementId>, markdown: impl Into<SharedString>) -> TextView {
    TextView::markdown(id, markdown)
        .selectable(true)
        .plugin(MentionPlugin)
        .image_source(resolve_image)
        .on_link_click(|url, _, _, cx| note(cx, format!("rich text: link clicked {url}")))
}

pub struct RichGate {
    messages: Vec<(SharedString, SharedString)>,
    png: PathBuf,
    gif: PathBuf,
    http_base: String,
    https_image: Option<String>,
}

impl RichGate {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let assets = cx.global::<Assets>();
        let png = assets.png.clone();
        let gif = assets.gif.clone();
        let http_base = assets.http_base.clone();
        let https_image = assets.https_image.clone();

        let inline = "**Bold**, *italic*, ~~strikethrough~~, `inline code`, a \
            [link to the docs](https://example.org/docs), a mention [Luca](mention:Luca), \
            emoji 🎉 👍🏽 🇧🇷 ❤️ and plain text that is long enough to wrap onto a second line \
            when the window is narrow, so selection across lines can be tested."
            .to_string();
        let code = "The export worker ran out of memory:\n\n```rust\nfn mix_tracks(tracks: &[Track]) -> Mixdown {\n    tracks.iter().filter(|t| !t.muted) // skip silent tracks\n        .map(|t| t.resample(48_000)).collect()\n}\n```\n\n```toml\n[retention]\ntracks_days = 30\n```".to_string();
        let blocks = "> Should recordings keep one track per speaker,\n> or only the mixdown?\n\n1. First ordered item\n2. Second ordered item\n   - nested bullet\n   - another nested bullet\n3. Third ordered item\n\n- [x] done task\n- [ ] open task".to_string();
        let images = format!(
            "Inline in a line of text: ![file png](file://{png}) and ![http png]({http}/sample.png) and ![file gif](file://{gif})\n\nImage from a file, alone in a paragraph:\n\n![file png](file://{png})\n\nImage from HTTP, alone in a paragraph:\n\n![http png]({http}/sample.png)\n\nAnimated GIF from a file, alone in a paragraph:\n\n![file gif](file://{gif})",
            png = png.display(),
            http = http_base,
            gif = gif.display()
        );

        Self {
            messages: vec![
                ("Ana".into(), inline.into()),
                ("Bruno".into(), code.into()),
                ("Diego".into(), blocks.into()),
                ("Carla".into(), images.into()),
            ],
            png,
            gif,
            http_base,
            https_image,
        }
    }
}

impl Render for RichGate {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let tile = |label: &'static str, source: ImageSource, id: &'static str| {
            v_flex()
                .gap(u(4.0))
                .child(div().text_size(u(12.0)).text_color(Palette::muted()).child(label))
                .child(
                    img(source)
                        .id(id)
                        .w(u(160.0))
                        .h(u(90.0))
                        .object_fit(ObjectFit::Contain),
                )
        };

        div()
            .id("rich-scroll")
            .size_full()
            .overflow_y_scroll()
            .p(u(16.0))
            .child(
                v_flex()
                    .max_w(u(760.0))
                    .gap(u(14.0))
                    .children(self.messages.iter().enumerate().map(|(index, (author, text))| {
                        v_flex()
                            .gap(u(2.0))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(author.clone()),
                            )
                            .child(message_view(("rich", index), text.clone()))
                    }))
                    .child(
                        div()
                            .text_size(u(12.0))
                            .text_color(Palette::muted())
                            .child("gpui img() elements, outside Markdown:"),
                    )
                    .child(
                        h_flex()
                            .gap(u(12.0))
                            .child(tile("file png", self.png.clone().into(), "img-file"))
                            .child(tile(
                                "http png",
                                format!("{}/sample.png", self.http_base).into(),
                                "img-http",
                            ))
                            .child(tile("file gif", self.gif.clone().into(), "img-gif"))
                            .child(tile(
                                "http gif",
                                format!("{}/anim.gif", self.http_base).into(),
                                "img-http-gif",
                            ))
                            .when_some(self.https_image.clone(), |this, url| {
                                this.child(tile("https image", url.into(), "img-https"))
                            }),
                    ),
            )
    }
}
