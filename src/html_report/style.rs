/// Returns the CSS content for the HTML report (everything inside `<style>...</style>`).
pub(super) fn css() -> &'static str {
    include_str!("../css/report.css")
}

/// Returns the JavaScript content for the HTML report (everything inside `<script>...</script>`).
///
/// Note: the JS uses literal braces, so the caller must NOT pass this through `format!`.
/// Instead it should be written directly via `write!` or string concatenation.
pub(super) fn js() -> &'static str {
    include_str!("../js/report.js")
}
