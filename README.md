# fetlife-action-text

Parses [Lexxy](https://github.com/basecamp/lexxy) / Action Text HTML into plain text, clipped HTML previews and a
content analysis (links, alt texts, attachments, images). Attachments are never resolved: callers that need
mention or picture text look the attachments up themselves and pass their texts back in document order.

```toml
[dependencies]
fetlife-action-text = { git = "https://github.com/fetlife/action-text", tag = "v0.1.1" }
```

```rust
use fetlife_action_text::RichText;

let document = RichText::parse("<p>Bring <em>rope</em></p><ul><li>and tea</li></ul>");
assert_eq!(document.plain_text(), "Bring rope\n\n• and tea");
```

## Releasing

Tag the commit (`v0.2.0`, …) and bump the `tag` in each consumer's `Cargo.toml`.
