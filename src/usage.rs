use regex::Regex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

/// A function/method definition within a crate.
#[derive(Debug)]
struct FnDef {
    /// Number of non-blank, non-comment lines in the body.
    loc: usize,
    /// Names of other functions/methods called from this body.
    callees: Vec<String>,
}

/// Result of reachability analysis within a dependency crate.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsageProfile {
    /// Total LOC in the fat dependency crate.
    pub total_loc: usize,
    /// LOC reachable from the entry points used by the intermediate crate.
    pub reachable_loc: usize,
    /// Number of function/method definitions in the crate.
    pub total_fns: usize,
    /// Number of reachable function/method definitions.
    pub reachable_fns: usize,
    /// The entry point symbols we traced from.
    pub entry_points: Vec<String>,
}

impl UsageProfile {
    pub fn unknown() -> Self {
        Self {
            total_loc: 0,
            reachable_loc: 0,
            total_fns: 0,
            reachable_fns: 0,
            entry_points: Vec::new(),
        }
    }

    /// Usage ratio: what fraction of the crate's code is reachable.
    pub fn usage_ratio(&self) -> f64 {
        if self.total_loc == 0 {
            return 0.0;
        }
        self.reachable_loc as f64 / self.total_loc as f64
    }
}

/// Build a usage profile for a fat dependency crate.
///
/// Given:
/// - The .rs files of the fat dep
/// - The entry point symbols (distinct API items used by the intermediate crate)
///
/// Traces the intra-crate call graph from those entry points and measures
/// how much code is reachable.
pub fn analyze_usage(rs_files: &[PathBuf], entry_points: &[String]) -> UsageProfile {
    if rs_files.is_empty() || entry_points.is_empty() {
        let total_loc = count_code_lines(rs_files);
        return UsageProfile {
            total_loc,
            reachable_loc: 0,
            total_fns: 0,
            reachable_fns: 0,
            entry_points: entry_points.to_vec(),
        };
    }

    // Phase 1: Extract all function/method definitions.
    let defs = extract_definitions(rs_files);
    let total_fns = defs.len();

    // Phase 2: BFS from entry points through the call graph.
    let reachable = trace_reachable(&defs, entry_points);
    let reachable_loc: usize = reachable.iter().filter_map(|name| defs.get(name)).map(|d| d.loc).sum();
    let reachable_fns = reachable.len();

    // Include non-function code (struct defs, constants, etc.) as baseline.
    let file_total_loc = count_code_lines(rs_files);

    UsageProfile {
        total_loc: file_total_loc,
        reachable_loc,
        total_fns,
        reachable_fns,
        entry_points: entry_points.to_vec(),
    }
}

/// Count non-blank, non-comment lines across files.
fn count_code_lines(rs_files: &[PathBuf]) -> usize {
    let mut count = 0;
    for path in rs_files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with("//") {
                count += 1;
            }
        }
    }
    count
}

/// Max total source bytes to analyze per crate. Skip huge crates.
const MAX_ANALYSIS_BYTES: u64 = 200_000;

/// Extract function and method definitions from source files.
/// Returns a map from name -> FnDef.
fn extract_definitions(rs_files: &[PathBuf]) -> HashMap<String, FnDef> {
    // Skip crates that are too large to analyze quickly.
    let total_bytes: u64 = rs_files
        .iter()
        .filter_map(|p| p.metadata().ok())
        .map(|m| m.len())
        .sum();
    if total_bytes > MAX_ANALYSIS_BYTES {
        return HashMap::new();
    }
    // Match fn definitions: `fn name(`, `pub fn name(`, `pub(crate) fn name(`, etc.
    let fn_re = Regex::new(r"(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+(\w+)\s*[<(]").unwrap();
    // Match impl blocks: `impl Foo {` or `impl Foo for Bar {`
    let impl_re = Regex::new(r"impl(?:<[^>]*>)?\s+(?:\w+\s+for\s+)?(\w+)").unwrap();

    let mut defs: HashMap<String, FnDef> = HashMap::new();

    for path in rs_files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.lines().collect();
        let mut current_impl: Option<String> = None;
        let mut brace_depth: i32 = 0;
        let mut impl_brace_depth: Option<i32> = None;

        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") {
                continue;
            }

            // Track brace depth for impl block scoping.
            let opens = trimmed.chars().filter(|c| *c == '{').count() as i32;
            let closes = trimmed.chars().filter(|c| *c == '}').count() as i32;

            // Detect impl block start.
            if let Some(caps) = impl_re.captures(trimmed) {
                if trimmed.contains('{') || (i + 1 < lines.len() && lines[i + 1].trim().starts_with('{')) {
                    current_impl = Some(caps[1].to_string());
                    impl_brace_depth = Some(brace_depth);
                }
            }

            // Detect fn definition.
            if let Some(caps) = fn_re.captures(trimmed) {
                let fn_name = caps[1].to_string();
                let qualified_name = match &current_impl {
                    Some(impl_name) => format!("{impl_name}::{fn_name}"),
                    None => fn_name.clone(),
                };

                // Extract the function body (approximate: count until braces balance).
                let (body_loc, body_text) = extract_fn_body(&lines, i);

                // Find callees: identifiers in the body that might be function calls.
                let callees = extract_callees(&body_text);

                defs.insert(
                    qualified_name.clone(),
                    FnDef {
                        loc: body_loc,
                        callees,
                    },
                );

                // Also register under the short name for matching.
                if current_impl.is_some() && !defs.contains_key(&fn_name) {
                    defs.entry(fn_name).or_insert_with(|| FnDef {
                        loc: 0,
                        callees: Vec::new(),
                    });
                    // Just create the short alias; the real def is under qualified_name.
                }
            }

            brace_depth += opens - closes;

            // Check if we've left the impl block.
            if let Some(ibd) = impl_brace_depth {
                if brace_depth <= ibd {
                    current_impl = None;
                    impl_brace_depth = None;
                }
            }
        }
    }

    defs
}

/// Extract the body of a function starting at line `start_idx`.
/// Returns (line count, body text).
fn extract_fn_body(lines: &[&str], start_idx: usize) -> (usize, String) {
    let mut depth: i32 = 0;
    let mut started = false;
    let mut loc = 0;
    let mut body = String::new();

    for line in &lines[start_idx..] {
        let trimmed = line.trim();

        let opens = trimmed.chars().filter(|c| *c == '{').count() as i32;
        let closes = trimmed.chars().filter(|c| *c == '}').count() as i32;

        if opens > 0 {
            started = true;
        }

        if started {
            if !trimmed.is_empty() && !trimmed.starts_with("//") {
                loc += 1;
            }
            body.push_str(trimmed);
            body.push('\n');

            depth += opens - closes;
            if depth <= 0 {
                break;
            }
        }

        // Safety: don't scan more than 500 lines for a single function.
        if loc > 500 {
            break;
        }
    }

    (loc, body)
}

/// Extract potential callee names from a function body.
/// Looks for patterns like `name(` and `Self::name(` and `name::func(`.
fn extract_callees(body: &str) -> Vec<String> {
    let call_re = Regex::new(r"\b(\w+)\s*[(<]").unwrap();
    let method_re = Regex::new(r"(?:self\.|Self::|\w+::)(\w+)\s*[(<]").unwrap();

    let mut callees = HashSet::new();

    for caps in call_re.captures_iter(body) {
        let name = &caps[1];
        // Filter out keywords and common non-function identifiers.
        if !is_keyword(name) && name.len() > 1 {
            callees.insert(name.to_string());
        }
    }

    for caps in method_re.captures_iter(body) {
        let name = &caps[1];
        if !is_keyword(name) && name.len() > 1 {
            callees.insert(name.to_string());
        }
    }

    callees.into_iter().collect()
}

fn is_keyword(s: &str) -> bool {
    matches!(
        s,
        "fn" | "let" | "if" | "else" | "match" | "for" | "while" | "loop" | "return"
            | "break" | "continue" | "struct" | "enum" | "impl" | "trait" | "type"
            | "pub" | "use" | "mod" | "crate" | "self" | "super" | "where" | "as"
            | "in" | "ref" | "mut" | "const" | "static" | "unsafe" | "async" | "await"
            | "move" | "dyn" | "true" | "false" | "Some" | "None" | "Ok" | "Err"
            | "Self" | "Box" | "Vec" | "String" | "Option" | "Result"
    )
}

/// BFS from entry points through the call graph.
/// Returns the set of reachable definition names.
fn trace_reachable(defs: &HashMap<String, FnDef>, entry_points: &[String]) -> HashSet<String> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    // Seed with entry points — try both short and qualified names.
    for ep in entry_points {
        // Try exact match.
        if defs.contains_key(ep) {
            if visited.insert(ep.clone()) {
                queue.push_back(ep.clone());
            }
        }
        // Try as a method: look for any "Type::ep" pattern.
        for name in defs.keys() {
            if name.ends_with(&format!("::{ep}")) || name == ep {
                if visited.insert(name.clone()) {
                    queue.push_back(name.clone());
                }
            }
        }
    }

    // BFS through callees.
    while let Some(current) = queue.pop_front() {
        if let Some(def) = defs.get(&current) {
            for callee in &def.callees {
                // Try exact match.
                if defs.contains_key(callee) && visited.insert(callee.clone()) {
                    queue.push_back(callee.clone());
                }
                // Try qualified matches.
                let qualified_matches: Vec<String> = defs
                    .keys()
                    .filter(|k| k.ends_with(&format!("::{callee}")) && !visited.contains(*k))
                    .cloned()
                    .collect();
                for qm in qualified_matches {
                    if visited.insert(qm.clone()) {
                        queue.push_back(qm);
                    }
                }
            }
        }
    }

    visited
}
