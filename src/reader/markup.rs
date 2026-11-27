//! XHTML as the HTML parser needs it.

/// Rewrite XML self-closing tags of non-void elements (`<p class="x"/>`,
/// `<a id="x"/>`) as an explicit start/end pair. Shared with
/// [`hanstat::extract::html::expand_self_closing`].
pub use hanstat::extract::html::expand_self_closing;

#[cfg(test)]
mod tests {
    #[test]
    fn self_closing() {
        assert_eq!(
            super::expand_self_closing(r#"<p class="a"/><br/><img src="x"/><a id="q/>"/>"#),
            r#"<p class="a"></p><br/><img src="x"/><a id="q/>"></a>"#
        );
    }
}
