//! lf — the lowfat filter DSL parser.
//!
//! Parses `.lf` files into a [`RuleSet`]. Execution lives elsewhere
//! (Task 2+). The DSL is line-oriented and indentation-sensitive; we
//! avoid INDENT/DEDENT tokens by working directly on `(indent, text)`
//! pairs, which keeps the parser short and the error messages tied to
//! source line numbers.

use crate::level::Level;
use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;

// ──────────────────────────────────────────────────────────────────
// AST
// ──────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct RuleSet {
    pub defines: Vec<Define>,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone)]
pub struct Define {
    pub name: String,
    pub params: Vec<String>,
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub sub: SubPattern,
    pub level: LevelPattern,
    pub ops: Vec<Op>,
    pub line_no: usize,
}

#[derive(Debug, Clone)]
pub enum SubPattern {
    Star,
    Alt(Vec<String>),
}

#[derive(Debug, Clone)]
pub enum LevelPattern {
    Star,
    Specific(Level),
}

#[derive(Debug, Clone)]
pub enum Op {
    Keep(PatternRegex),
    Drop(PatternRegex),
    Head(HeadArg),
    Tail(HeadArg),
    Or(String),
    OrShell(String),
    Shell(String),
    Python(String),
    Raw,
    MacroCall {
        name: String,
        args: Vec<MacroArg>,
    },
    Split {
        delimiter: PatternRegex,
        pre: Vec<Op>,
        post: Vec<Op>,
    },
    /// `if` / `elif` / `else` cascade — first matching branch runs.
    Cascade(Vec<Branch>),
}

/// One arm of an [`Op::Cascade`]. `guard: None` is the `else` arm.
#[derive(Debug, Clone)]
pub struct Branch {
    pub guard: Option<Guard>,
    pub ops: Vec<Op>,
}

/// A guard is an AND of atoms — `if level ultra and --stat:`.
#[derive(Debug, Clone)]
pub struct Guard {
    pub atoms: Vec<Atom>,
}

/// One closed-vocabulary condition inside a [`Guard`].
#[derive(Debug, Clone)]
pub enum Atom {
    Exit(ExitMatch),
    Level(Level),
    Flag(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitMatch {
    Ok,
    Failed,
}

#[derive(Debug, Clone)]
pub struct PatternRegex {
    pub source: String,
    pub compiled: Regex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadArg {
    Number(usize),
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroArg {
    Number(usize),
    String(String),
}

// ──────────────────────────────────────────────────────────────────
// Selection
// ──────────────────────────────────────────────────────────────────

impl RuleSet {
    /// First-match-wins. Returns `None` when no rule matches.
    pub fn select(&self, sub: &str, level: Level) -> Option<&Rule> {
        self.rules.iter().find(|r| r.matches(sub, level))
    }

    pub fn find_define(&self, name: &str) -> Option<&Define> {
        self.defines.iter().find(|d| d.name == name)
    }
}

impl Rule {
    pub fn matches(&self, sub: &str, level: Level) -> bool {
        let sub_ok = match &self.sub {
            SubPattern::Star => true,
            SubPattern::Alt(alts) => alts.iter().any(|a| glob_match(a, sub)),
        };
        let lvl_ok = match &self.level {
            LevelPattern::Star => true,
            LevelPattern::Specific(l) => *l == level,
        };
        sub_ok && lvl_ok
    }
}

// ──────────────────────────────────────────────────────────────────
// Line preprocessing
// ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Line {
    indent: usize,
    text: String, // trimmed of leading/trailing whitespace; "" if blank
    raw: String,  // original line, no trailing newline
    line_no: usize,
    /// Blank or starts with `#` at top-level. Meta lines are skipped by
    /// the structural parser but preserved as-is in block bodies.
    is_meta: bool,
}

fn split_lines(input: &str) -> Vec<Line> {
    input
        .split('\n')
        .enumerate()
        .map(|(i, raw_line)| {
            let raw = raw_line.trim_end_matches('\r').to_string();
            let stripped = raw.trim_start();
            let indent = raw.len() - stripped.len();
            let text = stripped.trim_end().to_string();
            let is_meta = text.is_empty() || text.starts_with('#');
            Line {
                indent,
                text,
                raw,
                line_no: i + 1,
                is_meta,
            }
        })
        .collect()
}

// ──────────────────────────────────────────────────────────────────
// Parser
// ──────────────────────────────────────────────────────────────────

const OP_KEYWORDS: &[&str] = &[
    "keep",
    "drop",
    "head",
    "tail",
    "or",
    "or-shell:",
    "else",
    "else-shell:",
    "shell:",
    "python:",
    "split",
    "raw",
    "passthrough",
    "if",
    "elif",
    "match",
];

pub fn parse(input: &str) -> Result<RuleSet> {
    let lines = split_lines(input);
    let macro_names = collect_macro_names(&lines);
    let mut p = Parser {
        lines: &lines,
        pos: 0,
        macro_names,
    };
    p.parse_ruleset()
}

fn collect_macro_names(lines: &[Line]) -> Vec<String> {
    let mut names = Vec::new();
    for l in lines {
        if l.is_meta {
            continue;
        }
        if let Some(rest) = l.text.strip_prefix("define ") {
            let end = rest
                .find(|c: char| c == '(' || c == ':' || c.is_whitespace())
                .unwrap_or(rest.len());
            let name = rest[..end].trim().to_string();
            if !name.is_empty() {
                names.push(name);
            }
        }
    }
    names
}

struct Parser<'a> {
    lines: &'a [Line],
    pos: usize,
    macro_names: Vec<String>,
}

impl<'a> Parser<'a> {
    /// Advance past meta lines and return the next structural line without
    /// consuming it.
    fn peek_significant(&mut self) -> Option<&'a Line> {
        while let Some(l) = self.lines.get(self.pos) {
            if l.is_meta {
                self.pos += 1;
            } else {
                return Some(l);
            }
        }
        None
    }

    fn advance(&mut self) -> Option<&'a Line> {
        let l = self.lines.get(self.pos);
        if l.is_some() {
            self.pos += 1;
        }
        l
    }

    fn is_macro(&self, name: &str) -> bool {
        self.macro_names.iter().any(|n| n == name)
    }

    // ── top-level ────────────────────────────────────────────────

    fn parse_ruleset(&mut self) -> Result<RuleSet> {
        let mut rs = RuleSet::default();
        while let Some(line) = self.peek_significant() {
            if line.indent != 0 {
                bail!("line {}: unexpected indent at top level", line.line_no);
            }
            if line.text.starts_with("define ") {
                let d = self.parse_define()?;
                rs.defines.push(d);
            } else {
                let r = self.parse_rule()?;
                rs.rules.push(r);
            }
        }
        Ok(rs)
    }

    fn parse_define(&mut self) -> Result<Define> {
        let header = self.advance().unwrap();
        let line_no = header.line_no;
        let rest = header
            .text
            .strip_prefix("define ")
            .ok_or_else(|| anyhow!("line {}: expected `define`", line_no))?;
        let (name, params, after_paren) =
            parse_define_header(rest).with_context(|| format!("line {line_no}"))?;
        if !after_paren.starts_with(':') {
            bail!(
                "line {}: expected `:` after define header, got `{}`",
                line_no,
                after_paren
            );
        }
        let trailing = after_paren[1..].trim();
        if !trailing.is_empty() {
            bail!(
                "line {}: one-line `define` body not supported (use indented body)",
                line_no
            );
        }
        let ops = self.parse_indented_ops(header.indent)?;
        if ops.is_empty() {
            bail!("line {}: `define {}` has empty body", line_no, name);
        }
        Ok(Define { name, params, ops })
    }

    fn parse_rule(&mut self) -> Result<Rule> {
        let header = self.advance().unwrap();
        let line_no = header.line_no;
        let parent_indent = header.indent;
        let colon_pos = header
            .text
            .find(':')
            .ok_or_else(|| anyhow!("line {}: missing `:` in rule header", line_no))?;
        let selector = &header.text[..colon_pos];
        let after = &header.text[colon_pos + 1..];
        let (sub, level) =
            parse_selector(selector).with_context(|| format!("line {line_no}"))?;

        let mut ops = Vec::new();
        let inline = after.trim();
        if !inline.is_empty() {
            // Inline ops after `:` are always a pipeline (v1 form).
            ops.extend(self.parse_inline_ops(inline, line_no)?);
            ops.extend(self.parse_indented_ops(parent_indent)?);
        } else {
            // An indented body may be a pipeline or an if/elif/else cascade.
            ops = self.parse_body(parent_indent)?;
        }

        if ops.is_empty() {
            bail!("line {}: rule has no ops", line_no);
        }
        Ok(Rule {
            sub,
            level,
            ops,
            line_no,
        })
    }

    // ── op chains ────────────────────────────────────────────────

    /// Parse op-lines strictly deeper-indented than `parent_indent`.
    /// Stops at first significant line whose indent <= parent_indent.
    fn parse_indented_ops(&mut self, parent_indent: usize) -> Result<Vec<Op>> {
        let mut ops = Vec::new();
        loop {
            let Some(line) = self.peek_significant() else {
                break;
            };
            if line.indent <= parent_indent {
                break;
            }
            let op = self.parse_op_line()?;
            ops.push(op);
        }
        Ok(ops)
    }

    /// An indented rule body: a plain pipeline, or a cascade when the
    /// first significant line opens with `if` (full cascade) or `match`
    /// (single-dimension sugar). Both desugar to `Op::Cascade`.
    fn parse_body(&mut self, parent_indent: usize) -> Result<Vec<Op>> {
        if let Some(line) = self.peek_significant() {
            if line.indent > parent_indent {
                if is_body_opener(&line.text, "if") {
                    let branches = self.parse_cascade(parent_indent)?;
                    return Ok(vec![Op::Cascade(branches)]);
                }
                if is_body_opener(&line.text, "match") {
                    let branches = self.parse_match(parent_indent)?;
                    return Ok(vec![Op::Cascade(branches)]);
                }
            }
        }
        self.parse_indented_ops(parent_indent)
    }

    /// Parse `if` / `elif`* / `else`? arms — all share one indent.
    fn parse_cascade(&mut self, parent_indent: usize) -> Result<Vec<Branch>> {
        let mut branches: Vec<Branch> = Vec::new();
        let mut arm_indent: Option<usize> = None;
        loop {
            let Some(line) = self.peek_significant() else {
                break;
            };
            if line.indent <= parent_indent {
                break;
            }
            match arm_indent {
                None => arm_indent = Some(line.indent),
                Some(ai) if line.indent != ai => break,
                Some(_) => {}
            }
            let line_no = line.line_no;
            // `else` is glued to its colon (`else:`), so take the leading
            // alphabetic run rather than the whitespace-delimited word.
            let kw: String = line
                .text
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            match kw.as_str() {
                "if" if branches.is_empty() => {}
                "elif" | "else" if !branches.is_empty() => {}
                "if" => bail!("line {}: unexpected `if` — cascade already open", line_no),
                "elif" | "else" => {
                    bail!("line {}: `{}` without a leading `if`", line_no, kw)
                }
                _ => break,
            }
            let branch = self.parse_branch(&kw)?;
            let is_else = branch.guard.is_none();
            branches.push(branch);
            if is_else {
                break; // `else` is always the last arm
            }
        }
        Ok(branches)
    }

    /// Parse one cascade arm: `<if|elif|else> <guard>:` then inline or
    /// indented ops.
    fn parse_branch(&mut self, head: &str) -> Result<Branch> {
        let line = self.advance().unwrap();
        let line_no = line.line_no;
        let indent = line.indent;
        let rest = line.text[head.len()..].trim_start();
        let colon = rest
            .find(':')
            .ok_or_else(|| anyhow!("line {}: missing `:` in `{}` arm", line_no, head))?;
        let guard_str = rest[..colon].trim();
        let after = rest[colon + 1..].trim();
        let guard = if head == "else" {
            if !guard_str.is_empty() {
                bail!("line {}: `else` takes no guard", line_no);
            }
            None
        } else {
            Some(parse_guard(guard_str, line_no)?)
        };
        let ops = self.parse_arm_body(after, indent, line_no)?;
        if ops.is_empty() {
            bail!("line {}: `{}` arm has no ops", line_no, head);
        }
        Ok(Branch { guard, ops })
    }

    /// Body of one arm — used by `if`/`elif`/`else` and by `match` arms.
    /// Inline ops after `:` force a pipeline body; otherwise the body may
    /// be a nested cascade (`if` or `match`) or a plain indented pipeline.
    fn parse_arm_body(
        &mut self,
        inline: &str,
        indent: usize,
        line_no: usize,
    ) -> Result<Vec<Op>> {
        let mut ops = Vec::new();
        if !inline.is_empty() {
            ops.extend(self.parse_inline_ops(inline, line_no)?);
        }
        if ops.is_empty() {
            if let Some(child) = self.peek_significant() {
                if child.indent > indent {
                    if is_body_opener(&child.text, "if") {
                        return Ok(vec![Op::Cascade(self.parse_cascade(indent)?)]);
                    }
                    if is_body_opener(&child.text, "match") {
                        return Ok(vec![Op::Cascade(self.parse_match(indent)?)]);
                    }
                }
            }
        }
        ops.extend(self.parse_indented_ops(indent)?);
        Ok(ops)
    }

    /// Sugar for a single-dimension cascade.
    ///   match level:
    ///       ultra: head 30
    ///       lite:  head 200
    ///       else:  head 80
    /// desugars to `if level ultra: … elif level lite: … else: …`.
    /// The dimension is `level` or `exit`; flags require the full `if` form.
    fn parse_match(&mut self, parent_indent: usize) -> Result<Vec<Branch>> {
        let header = self.advance().unwrap();
        let line_no = header.line_no;
        // Accept `match`, `match:`, or `match <dim>:` uniformly. The
        // is_body_opener gate above guarantees text starts with "match".
        let rest = header
            .text
            .strip_prefix("match")
            .ok_or_else(|| anyhow!("line {}: expected `match`", line_no))?
            .trim_start();
        let colon = rest
            .find(':')
            .ok_or_else(|| anyhow!("line {}: missing `:` after match dimension", line_no))?;
        let dim_str = rest[..colon].trim();
        let trailing = rest[colon + 1..].trim();
        if !trailing.is_empty() {
            bail!(
                "line {}: `match` header doesn't take inline ops (got `{}`)",
                line_no,
                trailing
            );
        }
        let dim = parse_match_dim(dim_str, line_no)?;

        let mut branches: Vec<Branch> = Vec::new();
        let mut arm_indent: Option<usize> = None;
        loop {
            let Some(line) = self.peek_significant() else {
                break;
            };
            if line.indent <= parent_indent {
                break;
            }
            match arm_indent {
                None => arm_indent = Some(line.indent),
                Some(ai) if line.indent != ai => break,
                Some(_) => {}
            }
            let branch = self.parse_match_arm(dim)?;
            let is_else = branch.guard.is_none();
            branches.push(branch);
            if is_else {
                break;
            }
        }

        if branches.is_empty() {
            bail!("line {}: `match` has no arms", line_no);
        }
        Ok(branches)
    }

    /// One `match` arm: `<value>: <ops>` or `else: <ops>`. Builds the
    /// guard atom by interpreting `<value>` against the captured `dim`.
    fn parse_match_arm(&mut self, dim: MatchDim) -> Result<Branch> {
        let line = self.advance().unwrap();
        let line_no = line.line_no;
        let indent = line.indent;
        let colon = line
            .text
            .find(':')
            .ok_or_else(|| anyhow!("line {}: missing `:` in match arm", line_no))?;
        let value = line.text[..colon].trim();
        let after = line.text[colon + 1..].trim();

        let guard = if value == "else" {
            None
        } else {
            let atom = build_match_atom(dim, value, line_no)?;
            Some(Guard { atoms: vec![atom] })
        };

        let ops = self.parse_arm_body(after, indent, line_no)?;
        if ops.is_empty() {
            bail!("line {}: match arm `{}` has no ops", line_no, value);
        }
        Ok(Branch { guard, ops })
    }

    /// Parse a single op from the current significant line, advancing
    /// past any block bodies and sub-blocks the op consumes.
    fn parse_op_line(&mut self) -> Result<Op> {
        let line = self.advance().unwrap();
        let line_no = line.line_no;
        let indent = line.indent;
        let text = line.text.as_str();
        let (head, _) = split_first_word(text);

        match head {
            "keep" => {
                let rest = text[head.len()..].trim_start();
                Ok(Op::Keep(parse_regex_literal(rest, line_no)?))
            }
            "drop" => {
                let rest = text[head.len()..].trim_start();
                Ok(Op::Drop(parse_regex_literal(rest, line_no)?))
            }
            "head" => {
                let rest = text[head.len()..].trim();
                Ok(Op::Head(parse_head_arg(rest, line_no)?))
            }
            "tail" => {
                let rest = text[head.len()..].trim();
                Ok(Op::Tail(parse_head_arg(rest, line_no)?))
            }
            "or" | "else" => {
                let rest = text[head.len()..].trim_start();
                Ok(Op::Or(parse_string_literal(rest, line_no)?))
            }
            "or-shell:" | "else-shell:" => {
                let body = text[head.len()..].trim_start().to_string();
                if body.is_empty() {
                    bail!("line {}: `{}` requires a command", line_no, head);
                }
                Ok(Op::OrShell(body))
            }
            // `raw` is canonical; `passthrough` is a v0.5.0 legacy alias.
            "raw" | "passthrough" => Ok(Op::Raw),
            "shell:" => Ok(Op::Shell(self.parse_block_body(
                text,
                head,
                indent,
                line_no,
            )?)),
            "python:" => Ok(Op::Python(self.parse_block_body(
                text,
                head,
                indent,
                line_no,
            )?)),
            "split" => {
                let rest = text[head.len()..].trim_start();
                let delim = parse_regex_literal(rest, line_no)?;
                let (pre, post) = self.parse_split_branches(indent)?;
                if pre.is_empty() && post.is_empty() {
                    bail!(
                        "line {}: `split` needs at least one `pre:` or `post:` block",
                        line_no
                    );
                }
                Ok(Op::Split {
                    delimiter: delim,
                    pre,
                    post,
                })
            }
            name if self.is_macro(name) => {
                let rest = text[head.len()..].trim();
                let args = parse_macro_args(rest, line_no)?;
                Ok(Op::MacroCall {
                    name: name.to_string(),
                    args,
                })
            }
            _ => bail!("line {}: unknown op `{}`", line_no, head),
        }
    }

    /// Parse a `shell:` or `python:` body. Two forms:
    ///   inline: `shell: <command on rest of line>`
    ///   block:  `shell: |` then indented body lines until dedent.
    /// Body lines preserve internal blank lines and relative indentation.
    fn parse_block_body(
        &mut self,
        line_text: &str,
        head: &str,
        parent_indent: usize,
        line_no: usize,
    ) -> Result<String> {
        let after = line_text[head.len()..].trim_start();
        if after != "|" {
            if after.is_empty() {
                bail!(
                    "line {}: empty `{}` body (use `| <newline>` for block form)",
                    line_no,
                    head
                );
            }
            return Ok(after.to_string());
        }

        // Block form: scan lines until indent drops back to parent_indent.
        // Include blank lines that fall between body lines.
        let mut collected: Vec<&'a Line> = Vec::new();
        let mut base: Option<usize> = None;
        while let Some(l) = self.lines.get(self.pos) {
            if l.text.is_empty() {
                collected.push(l);
                self.pos += 1;
                continue;
            }
            if l.indent <= parent_indent {
                break;
            }
            if base.is_none() {
                base = Some(l.indent);
            }
            collected.push(l);
            self.pos += 1;
        }
        // Trim trailing blank lines (they belong to the gap, not the body).
        while collected.last().map_or(false, |l| l.text.is_empty()) {
            collected.pop();
        }
        if collected.is_empty() {
            bail!("line {}: `{}` block is empty", line_no, head);
        }
        let base = base.unwrap_or(parent_indent + 4);
        let dedented: Vec<String> = collected
            .iter()
            .map(|l| {
                if l.text.is_empty() {
                    String::new()
                } else if l.raw.len() >= base {
                    l.raw[base..].to_string()
                } else {
                    l.raw.trim_start().to_string()
                }
            })
            .collect();
        Ok(dedented.join("\n"))
    }

    /// After a `split /regex/`, consume any sibling `pre:` / `post:`
    /// blocks at the same indent.
    fn parse_split_branches(&mut self, parent_indent: usize) -> Result<(Vec<Op>, Vec<Op>)> {
        let mut pre = Vec::new();
        let mut post = Vec::new();
        loop {
            let Some(line) = self.peek_significant() else {
                break;
            };
            if line.indent != parent_indent {
                break;
            }
            match line.text.as_str() {
                "pre:" => {
                    self.advance();
                    pre = self.parse_indented_ops(parent_indent)?;
                }
                "post:" => {
                    self.advance();
                    post = self.parse_indented_ops(parent_indent)?;
                }
                _ => break,
            }
        }
        Ok((pre, post))
    }

    /// Parse multiple ops appearing on the same line (after a rule
    /// header's `:`). `shell:` / `python:` / `else-shell:` greedily
    /// consume rest of line; other ops yield to the next op keyword
    /// or macro name.
    fn parse_inline_ops(&self, text: &str, line_no: usize) -> Result<Vec<Op>> {
        let mut ops = Vec::new();
        let mut remaining = text.trim();
        while !remaining.is_empty() {
            let (head, _) = split_first_word(remaining);
            match head {
                "shell:" => {
                    let body = remaining[head.len()..].trim_start().to_string();
                    if body.is_empty() {
                        bail!("line {}: inline `shell:` needs a command", line_no);
                    }
                    ops.push(Op::Shell(body));
                    remaining = "";
                }
                "python:" => {
                    let body = remaining[head.len()..].trim_start().to_string();
                    if body.is_empty() {
                        bail!("line {}: inline `python:` needs a command", line_no);
                    }
                    ops.push(Op::Python(body));
                    remaining = "";
                }
                "or-shell:" | "else-shell:" => {
                    let body = remaining[head.len()..].trim_start().to_string();
                    if body.is_empty() {
                        bail!("line {}: inline `{}` needs a command", line_no, head);
                    }
                    ops.push(Op::OrShell(body));
                    remaining = "";
                }
                "raw" | "passthrough" => {
                    ops.push(Op::Raw);
                    remaining = remaining[head.len()..].trim_start();
                }
                "keep" | "drop" => {
                    let rest = remaining[head.len()..].trim_start();
                    let (re, after) = parse_regex_literal_and_rest(rest, line_no)?;
                    ops.push(if head == "keep" {
                        Op::Keep(re)
                    } else {
                        Op::Drop(re)
                    });
                    remaining = after.trim_start();
                }
                "head" | "tail" => {
                    let rest = remaining[head.len()..].trim_start();
                    let (arg_word, after) = take_word(rest);
                    let h = parse_head_arg(arg_word, line_no)?;
                    ops.push(if head == "head" {
                        Op::Head(h)
                    } else {
                        Op::Tail(h)
                    });
                    remaining = after.trim_start();
                }
                "or" | "else" => {
                    let rest = remaining[head.len()..].trim_start();
                    let (s, after) = parse_string_literal_and_rest(rest, line_no)?;
                    ops.push(Op::Or(s));
                    remaining = after.trim_start();
                }
                "split" => {
                    bail!(
                        "line {}: `split` cannot appear inline (needs pre:/post: blocks)",
                        line_no
                    )
                }
                name if self.is_macro(name) => {
                    let rest = remaining[head.len()..].trim_start();
                    let (args, after) =
                        parse_macro_args_until_op(rest, &self.macro_names, line_no)?;
                    ops.push(Op::MacroCall {
                        name: name.to_string(),
                        args,
                    });
                    remaining = after.trim_start();
                }
                _ => bail!("line {}: unknown op `{}` in inline chain", line_no, head),
            }
        }
        Ok(ops)
    }
}

// ──────────────────────────────────────────────────────────────────
// Sub-parsers (free functions, no Parser state)
// ──────────────────────────────────────────────────────────────────

/// True when `text` opens with `kw` followed by whitespace, a `:`, or
/// end of input — i.e. `kw` introduces a body construct rather than
/// being a prefix of some other word (`matching`, `iffy`).
fn is_body_opener(text: &str, kw: &str) -> bool {
    match text.strip_prefix(kw) {
        None => false,
        Some(rest) => rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == ':'),
    }
}

fn split_first_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    (&s[..end], &s[end..])
}

fn take_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    (&s[..end], &s[end..])
}

fn parse_selector(s: &str) -> Result<(SubPattern, LevelPattern)> {
    let s = s.trim();
    if s.is_empty() {
        bail!("empty selector");
    }
    let mut parts = s.splitn(2, ',');
    let sub_str = parts.next().unwrap().trim();
    let level_str = parts.next().map(|s| s.trim()).unwrap_or("*");

    let sub = if sub_str == "*" {
        SubPattern::Star
    } else {
        let alts: Vec<String> = sub_str
            .split('|')
            .map(|s| s.trim().to_string())
            .collect();
        if alts.iter().any(|a| a.is_empty()) {
            bail!("empty alternative in sub pattern `{}`", sub_str);
        }
        SubPattern::Alt(alts)
    };

    let level = if level_str == "*" {
        LevelPattern::Star
    } else {
        let lvl: Level = level_str.parse().map_err(|e: String| anyhow!(e))?;
        LevelPattern::Specific(lvl)
    };

    Ok((sub, level))
}

/// Glob match for subcommand selectors. `*` matches any run of chars
/// (including empty); no other metacharacters. With no `*` it is an
/// exact compare, so plain selectors behave exactly as in v1.
fn glob_match(pat: &str, text: &str) -> bool {
    match pat.find('*') {
        None => pat == text,
        Some(star) => {
            let prefix = &pat[..star];
            let rest = &pat[star + 1..];
            let Some(tail) = text.strip_prefix(prefix) else {
                return false;
            };
            if rest.is_empty() {
                return true;
            }
            (0..=tail.len())
                .filter(|&i| tail.is_char_boundary(i))
                .any(|i| glob_match(rest, &tail[i..]))
        }
    }
}

/// Parse a guard — an AND of atoms joined by ` and `.
fn parse_guard(s: &str, line_no: usize) -> Result<Guard> {
    let mut atoms = Vec::new();
    for part in s.split(" and ") {
        let part = part.trim();
        if part.is_empty() {
            bail!("line {}: empty guard", line_no);
        }
        atoms.push(parse_atom(part, line_no)?);
    }
    if atoms.is_empty() {
        bail!("line {}: empty guard", line_no);
    }
    Ok(Guard { atoms })
}

/// Parse one guard atom: `exit ok|failed`, `level ultra|full|lite`, or a
/// `--flag` / `-x`.
fn parse_atom(s: &str, line_no: usize) -> Result<Atom> {
    if s.starts_with('-') {
        return Ok(Atom::Flag(s.to_string()));
    }
    let mut words = s.split_whitespace();
    let dim = words.next().unwrap_or("");
    let val = words.next();
    if words.next().is_some() {
        bail!("line {}: guard `{}` has too many words", line_no, s);
    }
    match (dim, val) {
        ("exit", Some("ok")) => Ok(Atom::Exit(ExitMatch::Ok)),
        ("exit", Some("failed")) => Ok(Atom::Exit(ExitMatch::Failed)),
        ("exit", Some(v)) => {
            bail!("line {}: unknown exit value `{}` (expected ok|failed)", line_no, v)
        }
        ("exit", None) => bail!("line {}: `exit` guard needs a value (ok|failed)", line_no),
        ("level", Some(v)) => {
            let lvl: Level = v.parse().map_err(|e: String| anyhow!("line {line_no}: {e}"))?;
            Ok(Atom::Level(lvl))
        }
        ("level", None) => bail!("line {}: `level` guard needs a value", line_no),
        (other, _) => bail!(
            "line {}: unknown guard `{}` (expected `exit ...`, `level ...`, or a --flag)",
            line_no,
            other
        ),
    }
}

/// Closed set of dimensions a `match` header may switch on. Flags are
/// not a `match` dimension — their presence is binary, with no "values"
/// to enumerate, so they must use `if --flag: ...` instead.
#[derive(Copy, Clone)]
enum MatchDim {
    Level,
    Exit,
}

fn parse_match_dim(s: &str, line_no: usize) -> Result<MatchDim> {
    match s {
        "level" => Ok(MatchDim::Level),
        "exit" => Ok(MatchDim::Exit),
        "" => bail!("line {}: `match` needs a dimension (level|exit)", line_no),
        other => bail!(
            "line {}: unknown match dimension `{}` (expected level|exit; flags must use `if --flag:`)",
            line_no,
            other
        ),
    }
}

fn build_match_atom(dim: MatchDim, value: &str, line_no: usize) -> Result<Atom> {
    match dim {
        MatchDim::Level => {
            let lvl: Level = value
                .parse()
                .map_err(|e: String| anyhow!("line {line_no}: {e}"))?;
            Ok(Atom::Level(lvl))
        }
        MatchDim::Exit => match value {
            "ok" => Ok(Atom::Exit(ExitMatch::Ok)),
            "failed" => Ok(Atom::Exit(ExitMatch::Failed)),
            other => bail!(
                "line {}: unknown exit value `{}` (expected ok|failed)",
                line_no,
                other
            ),
        },
    }
}

fn parse_define_header(s: &str) -> Result<(String, Vec<String>, &str)> {
    let s = s.trim_start();
    let end = s
        .find(|c: char| c == '(' || c == ':' || c.is_whitespace())
        .unwrap_or(s.len());
    let name = s[..end].to_string();
    if name.is_empty() {
        bail!("define needs a name");
    }
    let rest = s[end..].trim_start();
    if let Some(rest) = rest.strip_prefix('(') {
        let close = rest
            .find(')')
            .ok_or_else(|| anyhow!("missing `)` in define params"))?;
        let params: Vec<String> = rest[..close]
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        Ok((name, params, rest[close + 1..].trim_start()))
    } else {
        Ok((name, Vec::new(), rest))
    }
}

fn parse_regex_literal(s: &str, line_no: usize) -> Result<PatternRegex> {
    let (re, after) = parse_regex_literal_and_rest(s, line_no)?;
    let after = after.trim();
    if !after.is_empty() {
        bail!(
            "line {}: unexpected trailing input after regex: `{}`",
            line_no,
            after
        );
    }
    Ok(re)
}

fn parse_regex_literal_and_rest(s: &str, line_no: usize) -> Result<(PatternRegex, &str)> {
    let s = s.trim_start();
    if !s.starts_with('/') {
        bail!(
            "line {}: expected `/regex/`, got `{}`",
            line_no,
            preview(s)
        );
    }
    let body = &s[1..];
    let mut src = String::new();
    let mut chars = body.char_indices().peekable();
    let mut end_byte: Option<usize> = None;
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            if let Some((_, n)) = chars.next() {
                if n == '/' {
                    src.push('/');
                } else {
                    src.push('\\');
                    src.push(n);
                }
            } else {
                bail!("line {}: trailing backslash in regex", line_no);
            }
        } else if c == '/' {
            end_byte = Some(i);
            break;
        } else {
            src.push(c);
        }
    }
    let end_byte = end_byte.ok_or_else(|| anyhow!("line {}: unterminated regex", line_no))?;
    let after = &body[end_byte + 1..];
    let compiled = Regex::new(&src)
        .map_err(|e| anyhow!("line {}: invalid regex `{}`: {}", line_no, src, e))?;
    Ok((
        PatternRegex {
            source: src,
            compiled,
        },
        after,
    ))
}

fn parse_string_literal(s: &str, line_no: usize) -> Result<String> {
    let (s, after) = parse_string_literal_and_rest(s, line_no)?;
    let after = after.trim();
    if !after.is_empty() {
        bail!(
            "line {}: unexpected trailing input after string: `{}`",
            line_no,
            after
        );
    }
    Ok(s)
}

fn parse_string_literal_and_rest(s: &str, line_no: usize) -> Result<(String, &str)> {
    let s = s.trim_start();
    if !s.starts_with('"') {
        bail!(
            "line {}: expected `\"...\"`, got `{}`",
            line_no,
            preview(s)
        );
    }
    let body = &s[1..];
    let mut out = String::new();
    let mut chars = body.char_indices();
    let mut end_byte: Option<usize> = None;
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            if let Some((_, n)) = chars.next() {
                match n {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            } else {
                bail!("line {}: trailing backslash in string", line_no);
            }
        } else if c == '"' {
            end_byte = Some(i);
            break;
        } else {
            out.push(c);
        }
    }
    let end_byte = end_byte.ok_or_else(|| anyhow!("line {}: unterminated string", line_no))?;
    let after = &body[end_byte + 1..];
    Ok((out, after))
}

fn parse_head_arg(s: &str, line_no: usize) -> Result<HeadArg> {
    let s = s.trim();
    if s == "auto" {
        return Ok(HeadArg::Auto);
    }
    s.parse::<usize>().map(HeadArg::Number).map_err(|_| {
        anyhow!(
            "line {}: expected number or `auto`, got `{}`",
            line_no,
            s
        )
    })
}

fn parse_macro_args(s: &str, line_no: usize) -> Result<Vec<MacroArg>> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        if rest.starts_with('"') {
            let (sv, after) = parse_string_literal_and_rest(rest, line_no)?;
            out.push(MacroArg::String(sv));
            rest = after.trim_start();
        } else {
            let (word, after) = take_word(rest);
            out.push(match word.parse::<usize>() {
                Ok(n) => MacroArg::Number(n),
                Err(_) => MacroArg::String(word.to_string()),
            });
            rest = after.trim_start();
        }
    }
    Ok(out)
}

fn parse_macro_args_until_op<'a>(
    s: &'a str,
    macro_names: &[String],
    line_no: usize,
) -> Result<(Vec<MacroArg>, &'a str)> {
    let mut out = Vec::new();
    let mut rest = s.trim_start();
    while !rest.is_empty() {
        let (word, _) = take_word(rest);
        if OP_KEYWORDS.contains(&word) || macro_names.iter().any(|n| n == word) {
            break;
        }
        if rest.starts_with('"') {
            let (sv, after) = parse_string_literal_and_rest(rest, line_no)?;
            out.push(MacroArg::String(sv));
            rest = after.trim_start();
        } else {
            let (w, after) = take_word(rest);
            out.push(match w.parse::<usize>() {
                Ok(n) => MacroArg::Number(n),
                Err(_) => MacroArg::String(w.to_string()),
            });
            rest = after.trim_start();
        }
    }
    Ok((out, rest))
}

fn preview(s: &str) -> &str {
    let n = s.char_indices().nth(40).map(|(i, _)| i).unwrap_or(s.len());
    &s[..n]
}

// ──────────────────────────────────────────────────────────────────
// Execution
// ──────────────────────────────────────────────────────────────────

use std::io::Write;
use std::process::{Command, Stdio};

/// Per-invocation context passed to the executor and propagated as env
/// vars to `shell:` / `python:` subprocesses.
#[derive(Debug, Clone)]
pub struct ExecCtx<'a> {
    pub sub: &'a str,
    pub level: Level,
    pub exit_code: i32,
    pub args: &'a [String],
}

/// Run the matching rule against `input` and return the filtered output.
/// If no rule matches, the input is returned unchanged (passthrough).
///
/// Non-empty output always ends in a newline, matching the convention
/// of shell tools like `echo` and `grep`.
pub fn execute(rs: &RuleSet, ctx: &ExecCtx, input: &str) -> Result<String> {
    let Some(rule) = rs.select(ctx.sub, ctx.level) else {
        return Ok(input.to_string());
    };
    let out = run_ops(&rule.ops, ctx, input, rs, &[])?;
    Ok(ensure_trailing_newline(out))
}

fn ensure_trailing_newline(mut s: String) -> String {
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// One stage's input/output stats, recorded by [`execute_explain`].
#[derive(Debug, Clone)]
pub struct StageRecord {
    pub op_desc: String,
    pub stdin_lines: usize,
    pub stdin_bytes: usize,
    pub stdout_lines: usize,
    pub stdout_bytes: usize,
    pub elapsed_us: u128,
}

#[derive(Debug, Default, Clone)]
pub struct ExplainTrace {
    /// Index into `RuleSet::rules` of the matched rule (None if no match).
    pub matched_rule: Option<usize>,
    pub stages: Vec<StageRecord>,
}

/// Like [`execute`] but records per-op stats. Only top-level ops are
/// recorded — macros and split sub-chains run silently. Adds ~µs of
/// overhead per op for line/byte counting; safe for interactive use,
/// avoid in tight loops.
pub fn execute_explain(
    rs: &RuleSet,
    ctx: &ExecCtx,
    input: &str,
) -> Result<(String, ExplainTrace)> {
    let mut trace = ExplainTrace::default();
    let Some((idx, rule)) = rs
        .rules
        .iter()
        .enumerate()
        .find(|(_, r)| r.matches(ctx.sub, ctx.level))
    else {
        return Ok((input.to_string(), trace));
    };
    trace.matched_rule = Some(idx);

    let raw = input.to_string();
    let mut state = input.to_string();
    for op in &rule.ops {
        let stdin_lines = state.lines().count();
        let stdin_bytes = state.len();
        let start = std::time::Instant::now();
        let new_state = apply_op(op, &state, &raw, ctx, rs, &[])?;
        let elapsed_us = start.elapsed().as_micros();
        trace.stages.push(StageRecord {
            op_desc: describe_op(op),
            stdin_lines,
            stdin_bytes,
            stdout_lines: new_state.lines().count(),
            stdout_bytes: new_state.len(),
            elapsed_us,
        });
        state = new_state;
    }
    Ok((ensure_trailing_newline(state), trace))
}

fn describe_op(op: &Op) -> String {
    match op {
        Op::Keep(p) => format!("keep /{}/", p.source),
        Op::Drop(p) => format!("drop /{}/", p.source),
        Op::Head(arg) => format!("head {}", describe_head(arg)),
        Op::Tail(arg) => format!("tail {}", describe_head(arg)),
        Op::Or(s) => format!("or {s:?}"),
        Op::OrShell(s) => format!("or-shell: {}", first_line(s)),
        Op::Raw => "raw".to_string(),
        Op::Cascade(branches) => format!("cascade ({} arms)", branches.len()),
        Op::Shell(s) => format!("shell: {}", first_line(s)),
        Op::Python(s) => {
            if has_pep723_header(s) {
                format!("python (uv): {}", first_line(s))
            } else {
                format!("python: {}", first_line(s))
            }
        }
        Op::MacroCall { name, args } => {
            let parts: Vec<String> = args
                .iter()
                .map(|a| match a {
                    MacroArg::Number(n) => n.to_string(),
                    MacroArg::String(s) => s.clone(),
                })
                .collect();
            if parts.is_empty() {
                name.clone()
            } else {
                format!("{name} {}", parts.join(" "))
            }
        }
        Op::Split { delimiter, .. } => format!("split /{}/", delimiter.source),
    }
}

fn describe_head(a: &HeadArg) -> String {
    match a {
        HeadArg::Number(n) => n.to_string(),
        HeadArg::Auto => "auto".into(),
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(60).collect()
}

fn run_ops(
    ops: &[Op],
    ctx: &ExecCtx,
    input: &str,
    rs: &RuleSet,
    macro_args: &[MacroArg],
) -> Result<String> {
    let raw = input.to_string();
    let mut state = input.to_string();
    for op in ops {
        state = apply_op(op, &state, &raw, ctx, rs, macro_args)?;
    }
    Ok(state)
}

fn apply_op(
    op: &Op,
    state: &str,
    raw: &str,
    ctx: &ExecCtx,
    rs: &RuleSet,
    macro_args: &[MacroArg],
) -> Result<String> {
    match op {
        Op::Keep(pat) => Ok(filter_lines(state, |l| pat.compiled.is_match(l))),
        Op::Drop(pat) => Ok(filter_lines(state, |l| !pat.compiled.is_match(l))),
        Op::Head(arg) => Ok(take_head(state, resolve_head(arg, ctx.level))),
        Op::Tail(arg) => Ok(take_tail(state, resolve_head(arg, ctx.level))),
        Op::Or(s) => Ok(if state.trim().is_empty() {
            s.clone()
        } else {
            state.to_string()
        }),
        Op::OrShell(cmd) => {
            if state.trim().is_empty() {
                let expanded = expand_args(cmd, macro_args);
                run_shell(&expanded, raw, ctx)
            } else {
                Ok(state.to_string())
            }
        }
        Op::Raw => Ok(state.to_string()),
        Op::Cascade(branches) => {
            for br in branches {
                let hit = match &br.guard {
                    None => true,
                    Some(g) => guard_matches(g, ctx),
                };
                if hit {
                    return run_ops(&br.ops, ctx, state, rs, macro_args);
                }
            }
            // No arm matched and no `else` — leave the stream untouched.
            Ok(state.to_string())
        }
        Op::Shell(cmd) => {
            let expanded = expand_args(cmd, macro_args);
            run_shell(&expanded, state, ctx)
        }
        Op::Python(body) => {
            let expanded = expand_args(body, macro_args);
            run_python(&expanded, state, ctx)
        }
        Op::MacroCall { name, args } => {
            let def = rs
                .find_define(name)
                .ok_or_else(|| anyhow!("undefined macro `{}`", name))?;
            if args.len() != def.params.len() {
                bail!(
                    "macro `{}` expects {} arg(s), got {}",
                    name,
                    def.params.len(),
                    args.len()
                );
            }
            run_ops(&def.ops, ctx, state, rs, args)
        }
        Op::Split {
            delimiter,
            pre,
            post,
        } => {
            let (a, b) = split_at_first_match(state, &delimiter.compiled);
            let pre_out = if pre.is_empty() {
                a
            } else {
                run_ops(pre, ctx, &a, rs, macro_args)?
            };
            let post_out = if post.is_empty() {
                b
            } else {
                run_ops(post, ctx, &b, rs, macro_args)?
            };
            Ok(join_nonempty(&pre_out, &post_out))
        }
    }
}

/// A guard holds when every atom holds (AND).
fn guard_matches(g: &Guard, ctx: &ExecCtx) -> bool {
    g.atoms.iter().all(|a| atom_matches(a, ctx))
}

fn atom_matches(a: &Atom, ctx: &ExecCtx) -> bool {
    match a {
        Atom::Exit(ExitMatch::Ok) => ctx.exit_code == 0,
        Atom::Exit(ExitMatch::Failed) => ctx.exit_code != 0,
        Atom::Level(l) => *l == ctx.level,
        Atom::Flag(f) => ctx.args.iter().any(|arg| arg == f),
    }
}

fn resolve_head(arg: &HeadArg, level: Level) -> usize {
    match arg {
        HeadArg::Number(n) => *n,
        HeadArg::Auto => level.head_limit(30),
    }
}

fn filter_lines(s: &str, mut keep: impl FnMut(&str) -> bool) -> String {
    s.lines()
        .filter(|l| keep(l))
        .collect::<Vec<_>>()
        .join("\n")
}

fn take_head(s: &str, n: usize) -> String {
    s.lines().take(n).collect::<Vec<_>>().join("\n")
}

fn take_tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Split input at the first line matching `re`. The matching line goes
/// into `post`. If no line matches, everything is `pre` and `post` is
/// empty.
fn split_at_first_match(s: &str, re: &Regex) -> (String, String) {
    let mut pre = String::new();
    let mut post = String::new();
    let mut in_post = false;
    for line in s.lines() {
        if !in_post && re.is_match(line) {
            in_post = true;
        }
        let buf = if in_post { &mut post } else { &mut pre };
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(line);
    }
    (pre, post)
}

fn join_nonempty(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => String::new(),
        (true, false) => b.to_string(),
        (false, true) => a.to_string(),
        (false, false) => format!("{a}\n{b}"),
    }
}

/// Replace `$1`..`$9` with macro positional args. Other `$NAME` tokens
/// (e.g. `$level`, `$sub`) are left intact so shell can expand them
/// from env vars.
fn expand_args(body: &str, args: &[MacroArg]) -> String {
    if args.is_empty() {
        return body.to_string();
    }
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'$' && i + 1 < bytes.len() {
            let n = bytes[i + 1];
            if n.is_ascii_digit() && n != b'0' {
                let idx = (n - b'0') as usize;
                if idx <= args.len() {
                    match &args[idx - 1] {
                        MacroArg::Number(v) => out.push_str(&v.to_string()),
                        MacroArg::String(v) => out.push_str(v),
                    }
                    i += 2;
                    continue;
                }
            }
        }
        out.push(c as char);
        i += 1;
    }
    out
}

fn run_shell(cmd: &str, stdin_data: &str, ctx: &ExecCtx) -> Result<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .env("level", ctx.level.to_string())
        .env("sub", ctx.sub)
        .env("exit", ctx.exit_code.to_string())
        .env("args", ctx.args.join(" "))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning sh")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_data.as_bytes())
            .context("writing to sh stdin")?;
    }

    let output = child.wait_with_output().context("waiting for sh")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "shell exited {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run_python(body: &str, stdin_data: &str, ctx: &ExecCtx) -> Result<String> {
    if has_pep723_header(body) {
        run_python_uv(body, stdin_data, ctx)
    } else {
        run_python_plain(body, stdin_data, ctx)
    }
}

fn has_pep723_header(body: &str) -> bool {
    body.lines()
        .any(|l| l.trim_start().starts_with("# /// script"))
}

fn run_python_plain(body: &str, stdin_data: &str, ctx: &ExecCtx) -> Result<String> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(body)
        .env("level", ctx.level.to_string())
        .env("sub", ctx.sub)
        .env("exit", ctx.exit_code.to_string())
        .env("args", ctx.args.join(" "))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning python3")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_data.as_bytes())
            .context("writing to python stdin")?;
    }
    let output = child.wait_with_output().context("waiting for python")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "python exited {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// PEP 723: write the body to a temp file and let `uv run --script` resolve
/// inline dependencies. Data flows via stdin to the script.
fn run_python_uv(body: &str, stdin_data: &str, ctx: &ExecCtx) -> Result<String> {
    let mut script = tempfile::Builder::new()
        .prefix("lowfat-lf-")
        .suffix(".py")
        .tempfile()
        .context("creating temp script file")?;
    script
        .write_all(body.as_bytes())
        .context("writing temp script")?;
    script.flush().ok();

    let path = script
        .path()
        .to_str()
        .ok_or_else(|| anyhow!("non-UTF8 temp path"))?
        .to_string();

    let mut child = Command::new("uv")
        .args(["run", "--script", &path])
        .env("level", ctx.level.to_string())
        .env("sub", ctx.sub)
        .env("exit", ctx.exit_code.to_string())
        .env("args", ctx.args.join(" "))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning uv (is `uv` installed?)")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_data.as_bytes())
            .context("writing to uv stdin")?;
    }
    let output = child.wait_with_output().context("waiting for uv")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "uv exited {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ──────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> RuleSet {
        parse(src).unwrap_or_else(|e| panic!("parse failed: {e}\n--- src ---\n{src}"))
    }

    #[test]
    fn empty_input() {
        let rs = parse_ok("");
        assert!(rs.rules.is_empty());
        assert!(rs.defines.is_empty());
    }

    #[test]
    fn comments_and_blanks_only() {
        let rs = parse_ok("# hi\n\n# more\n");
        assert!(rs.rules.is_empty());
    }

    #[test]
    fn simple_rule() {
        let rs = parse_ok(
            r#"
status:
    keep /foo/
    head 10
"#,
        );
        assert_eq!(rs.rules.len(), 1);
        let r = &rs.rules[0];
        assert!(matches!(&r.sub, SubPattern::Alt(a) if a == &["status".to_string()]));
        assert!(matches!(r.level, LevelPattern::Star));
        assert_eq!(r.ops.len(), 2);
        match &r.ops[0] {
            Op::Keep(p) => assert_eq!(p.source, "foo"),
            _ => panic!("expected Keep"),
        }
        assert!(matches!(r.ops[1], Op::Head(HeadArg::Number(10))));
    }

    #[test]
    fn sub_with_alternation_and_level() {
        let rs = parse_ok(
            r#"
build|check, ultra:
    head 15
"#,
        );
        let r = &rs.rules[0];
        match &r.sub {
            SubPattern::Alt(a) => assert_eq!(a, &["build".to_string(), "check".to_string()]),
            _ => panic!("expected Alt"),
        }
        assert!(matches!(r.level, LevelPattern::Specific(Level::Ultra)));
    }

    #[test]
    fn star_wildcards() {
        let rs = parse_ok(
            r#"
*:
    head 30
"#,
        );
        assert!(matches!(rs.rules[0].sub, SubPattern::Star));
        assert!(matches!(rs.rules[0].level, LevelPattern::Star));
    }

    #[test]
    fn else_string_fallback() {
        let rs = parse_ok(
            r#"
status:
    keep /^M /
    head 5
    else "clean"
"#,
        );
        match &rs.rules[0].ops[2] {
            Op::Or(s) => assert_eq!(s, "clean"),
            _ => panic!("expected Or"),
        }
    }

    #[test]
    fn shell_inline_and_block() {
        let rs = parse_ok(
            r#"
define a:
    shell: sed -E 's/x/y/'

define b:
    shell: |
        awk '
          BEGIN { n=0 }
          { print; n++ }
        '
"#,
        );
        match &rs.defines[0].ops[0] {
            Op::Shell(s) => assert_eq!(s, "sed -E 's/x/y/'"),
            _ => panic!("expected inline Shell"),
        }
        match &rs.defines[1].ops[0] {
            Op::Shell(s) => {
                assert!(s.starts_with("awk '"));
                assert!(s.contains("BEGIN { n=0 }"));
                assert!(s.contains("{ print; n++ }"));
            }
            _ => panic!("expected block Shell"),
        }
    }

    #[test]
    fn python_block_preserves_pep723_and_blanks() {
        let rs = parse_ok(
            r#"
define clean:
    python: |
        # /// script
        # dependencies = ["pyyaml>=6"]
        # ///
        import sys, yaml

        for d in yaml.safe_load_all(sys.stdin):
            print(d)
"#,
        );
        match &rs.defines[0].ops[0] {
            Op::Python(s) => {
                assert!(s.contains("# /// script"));
                assert!(s.contains("# dependencies = [\"pyyaml>=6\"]"));
                assert!(s.contains("import sys, yaml"));
                // Blank line between imports and loop preserved
                assert!(s.contains("yaml\n\nfor"));
                // Internal indent preserved (4 spaces under `for`)
                assert!(s.contains("    print(d)"));
            }
            _ => panic!("expected Python"),
        }
    }

    #[test]
    fn macro_call_with_args() {
        let rs = parse_ok(
            r#"
define compact(n):
    head 1

diff, ultra:
    compact 30
"#,
        );
        match &rs.rules[0].ops[0] {
            Op::MacroCall { name, args } => {
                assert_eq!(name, "compact");
                assert_eq!(args, &[MacroArg::Number(30)]);
            }
            _ => panic!("expected MacroCall"),
        }
    }

    #[test]
    fn inline_ops_after_rule_header() {
        let rs = parse_ok(
            r#"
define compact(n):
    head 1

diff, ultra:  compact 30  else-shell: awk 'NF' | head -50
"#,
        );
        let ops = &rs.rules[0].ops;
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Op::MacroCall { name, .. } if name == "compact"));
        match &ops[1] {
            Op::OrShell(s) => assert_eq!(s, "awk 'NF' | head -50"),
            _ => panic!("expected OrShell, got {:?}", &ops[1]),
        }
    }

    #[test]
    fn split_with_pre_and_post() {
        let rs = parse_ok(
            r#"
define ah:
    shell: cat

show:
    split /^diff /
    pre:
        keep /^commit /
        ah
    post:
        head 10
    head 100
"#,
        );
        let ops = &rs.rules[0].ops;
        assert_eq!(ops.len(), 2);
        match &ops[0] {
            Op::Split {
                delimiter,
                pre,
                post,
            } => {
                assert_eq!(delimiter.source, "^diff ");
                assert_eq!(pre.len(), 2);
                assert_eq!(post.len(), 1);
                assert!(matches!(&pre[0], Op::Keep(_)));
                assert!(matches!(&pre[1], Op::MacroCall { name, .. } if name == "ah"));
                assert!(matches!(post[0], Op::Head(HeadArg::Number(10))));
            }
            _ => panic!("expected Split"),
        }
        assert!(matches!(ops[1], Op::Head(HeadArg::Number(100))));
    }

    #[test]
    fn first_match_wins_selection() {
        let rs = parse_ok(
            r#"
diff, ultra:
    head 5

diff:
    head 20

*:
    head 30
"#,
        );
        let r = rs.select("diff", Level::Ultra).unwrap();
        assert!(matches!(r.ops[0], Op::Head(HeadArg::Number(5))));
        let r = rs.select("diff", Level::Full).unwrap();
        assert!(matches!(r.ops[0], Op::Head(HeadArg::Number(20))));
        let r = rs.select("status", Level::Ultra).unwrap();
        assert!(matches!(r.ops[0], Op::Head(HeadArg::Number(30))));
    }

    #[test]
    fn alternation_in_selector_matches() {
        let rs = parse_ok(
            r#"
build|check, ultra:
    head 15
"#,
        );
        assert!(rs.select("build", Level::Ultra).is_some());
        assert!(rs.select("check", Level::Ultra).is_some());
        assert!(rs.select("test", Level::Ultra).is_none());
        assert!(rs.select("build", Level::Full).is_none());
    }

    #[test]
    fn head_auto_keyword() {
        let rs = parse_ok(
            r#"
foo:
    head auto
"#,
        );
        assert!(matches!(rs.rules[0].ops[0], Op::Head(HeadArg::Auto)));
    }

    #[test]
    fn regex_with_escaped_slash() {
        let rs = parse_ok(
            r#"
foo:
    keep /a\/b/
"#,
        );
        match &rs.rules[0].ops[0] {
            Op::Keep(p) => assert_eq!(p.source, "a/b"),
            _ => panic!(),
        }
    }

    #[test]
    fn errors_on_unterminated_regex() {
        let err = parse("foo:\n    keep /abc\n").unwrap_err();
        assert!(err.to_string().contains("unterminated regex"), "got: {err}");
    }

    #[test]
    fn errors_on_unknown_op() {
        let err = parse("foo:\n    nonsense 1\n").unwrap_err();
        assert!(err.to_string().contains("unknown op"), "got: {err}");
    }

    #[test]
    fn errors_on_invalid_level() {
        let err = parse("foo, gigamax:\n    head 5\n").unwrap_err();
        // anyhow only renders the outermost message via Display; use {:#}
        // to walk the cause chain.
        let chain = format!("{err:#}");
        assert!(chain.contains("unknown level"), "got: {chain}");
    }

    #[test]
    fn errors_on_empty_rule_body() {
        let err = parse("foo:\nbar:\n    head 5\n").unwrap_err();
        assert!(err.to_string().contains("rule has no ops"), "got: {err}");
    }

    // ── full plugin files parse cleanly ──────────────────────────

    #[test]
    fn git_compact_plugin_parses() {
        let src = include_str!(
            "../../lowfat-plugin/embedded/git/git-compact/filter.lf"
        );
        let rs = parse_ok(src);
        // Defines: strip-trailers, abbrev-hash, compact-diff, drop-index-meta
        assert_eq!(rs.defines.len(), 4);
        let names: Vec<&str> = rs.defines.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["strip-trailers", "abbrev-hash", "compact-diff", "drop-index-meta"]);
        assert_eq!(rs.defines[2].params, vec!["limit".to_string()]);

        // Selection sanity
        assert!(rs.select("status", Level::Full).is_some());
        assert!(rs.select("diff", Level::Ultra).is_some());
        assert!(rs.select("diff", Level::Lite).is_some());
        assert!(rs.select("diff", Level::Full).is_some());
        assert!(rs.select("log", Level::Ultra).is_some());
        assert!(rs.select("show", Level::Ultra).is_some());
        assert!(rs.select("show", Level::Full).is_some());
        // Catch-all
        assert!(rs.select("nothing", Level::Full).is_some());

        // Show rule is now a level cascade.
        let show_full = rs.select("show", Level::Full).unwrap();
        assert!(matches!(&show_full.ops[0], Op::Cascade(_)));
    }

    // ── executor ─────────────────────────────────────────────────

    fn ctx<'a>(sub: &'a str, level: Level) -> ExecCtx<'a> {
        ExecCtx {
            sub,
            level,
            exit_code: 0,
            args: &[],
        }
    }

    #[test]
    fn exec_keep_drop_head_tail() {
        let rs = parse_ok(
            r#"
foo:
    keep /^a/
    drop /skip/
    head 3
"#,
        );
        let input = "alpha\nbeta\na-skip\namber\naxe\nakira\n";
        let out = execute(&rs, &ctx("foo", Level::Full), input).unwrap();
        assert_eq!(out, "alpha\namber\naxe\n");
    }

    #[test]
    fn exec_tail() {
        let rs = parse_ok(
            r#"
foo:
    tail 2
"#,
        );
        let out = execute(&rs, &ctx("foo", Level::Full), "a\nb\nc\nd").unwrap();
        assert_eq!(out, "c\nd\n");
    }

    #[test]
    fn exec_else_string_when_empty() {
        let rs = parse_ok(
            r#"
status:
    keep /^M /
    else "clean"
"#,
        );
        let out = execute(&rs, &ctx("status", Level::Full), "?? new.txt\n").unwrap();
        assert_eq!(out, "clean\n");
    }

    #[test]
    fn exec_else_string_passthrough_when_nonempty() {
        let rs = parse_ok(
            r#"
status:
    keep /^M /
    else "clean"
"#,
        );
        let out = execute(&rs, &ctx("status", Level::Full), "M file.txt\n").unwrap();
        assert_eq!(out, "M file.txt\n");
    }

    #[test]
    fn exec_no_match_passes_through() {
        let rs = parse_ok(
            r#"
foo:
    head 1
"#,
        );
        let input = "x\ny\nz";
        let out = execute(&rs, &ctx("other", Level::Full), input).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn exec_first_match_wins() {
        let rs = parse_ok(
            r#"
diff, ultra:
    head 1
diff:
    head 3
"#,
        );
        let input = "a\nb\nc\nd\n";
        let u = execute(&rs, &ctx("diff", Level::Ultra), input).unwrap();
        let f = execute(&rs, &ctx("diff", Level::Full), input).unwrap();
        assert_eq!(u, "a\n");
        assert_eq!(f, "a\nb\nc\n");
    }

    #[test]
    fn exec_head_auto_uses_level() {
        let rs = parse_ok(
            r#"
foo:
    head auto
"#,
        );
        let input: String = (1..=80).map(|i| format!("{i}\n")).collect();
        let u = execute(&rs, &ctx("foo", Level::Ultra), &input).unwrap();
        let f = execute(&rs, &ctx("foo", Level::Full), &input).unwrap();
        let l = execute(&rs, &ctx("foo", Level::Lite), &input).unwrap();
        assert_eq!(u.lines().count(), 15);
        assert_eq!(f.lines().count(), 30);
        assert_eq!(l.lines().count(), 60);
    }

    #[test]
    fn exec_shell_inline() {
        let rs = parse_ok(
            r#"
foo:
    shell: tr a-z A-Z
"#,
        );
        let out = execute(&rs, &ctx("foo", Level::Full), "hello\n").unwrap();
        assert_eq!(out.trim_end(), "HELLO");
    }

    #[test]
    fn exec_shell_block() {
        let rs = parse_ok(
            r#"
foo:
    shell: |
        awk '{ print NR, $0 }'
"#,
        );
        let out = execute(&rs, &ctx("foo", Level::Full), "a\nb\n").unwrap();
        assert_eq!(out.trim_end(), "1 a\n2 b");
    }

    #[test]
    fn exec_shell_sees_env_vars() {
        let rs = parse_ok(
            r#"
build:
    shell: printf '%s:%s' "$sub" "$level"
"#,
        );
        let out = execute(&rs, &ctx("build", Level::Ultra), "").unwrap();
        // ensure_trailing_newline normalizes shell output without a final \n
        assert_eq!(out, "build:ultra\n");
    }

    #[test]
    fn exec_else_shell_uses_raw_input() {
        let rs = parse_ok(
            r#"
diff:
    keep /^IMPOSSIBLE/
    else-shell: head -2
"#,
        );
        let out = execute(&rs, &ctx("diff", Level::Full), "x\ny\nz\n").unwrap();
        assert_eq!(out, "x\ny\n");
    }

    #[test]
    fn exec_macro_expansion_with_args() {
        let rs = parse_ok(
            r#"
define n-up(count):
    shell: head -$1

foo:
    n-up 2
"#,
        );
        let out = execute(&rs, &ctx("foo", Level::Full), "a\nb\nc\nd\n").unwrap();
        assert_eq!(out, "a\nb\n");
    }

    #[test]
    fn exec_split_pre_post() {
        let rs = parse_ok(
            r#"
show:
    split /^diff /
    pre:
        head 1
    post:
        head 2
"#,
        );
        let input = "commit abc\nAuthor: x\nDate: y\ndiff --git a b\n+line1\n+line2\n+line3\n";
        let out = execute(&rs, &ctx("show", Level::Full), input).unwrap();
        assert_eq!(out, "commit abc\ndiff --git a b\n+line1\n");
    }

    #[test]
    fn exec_split_no_match() {
        let rs = parse_ok(
            r#"
show:
    split /^diff /
    pre:
        head 2
    post:
        head 10
"#,
        );
        // No `diff ` line — everything goes to pre, post is empty.
        let out = execute(&rs, &ctx("show", Level::Full), "a\nb\nc\nd\n").unwrap();
        assert_eq!(out, "a\nb\n");
    }

    #[test]
    fn exec_macro_arg_count_mismatch_errors() {
        let rs = parse_ok(
            r#"
define needs-two(a, b):
    head 1

foo:
    needs-two 5
"#,
        );
        let err = execute(&rs, &ctx("foo", Level::Full), "x").unwrap_err();
        assert!(err.to_string().contains("expects 2 arg"), "got: {err}");
    }

    #[test]
    fn exec_python_plain_when_no_pep723() {
        // Skip if python3 not on PATH.
        if Command::new("python3").arg("--version").output().is_err() {
            eprintln!("skipping: python3 not available");
            return;
        }
        let rs = parse_ok(
            r#"
foo:
    python: |
        import sys
        for line in sys.stdin:
            print(line.upper(), end="")
"#,
        );
        let out = execute(&rs, &ctx("foo", Level::Full), "hello\nworld\n").unwrap();
        assert_eq!(out, "HELLO\nWORLD\n");
    }

    #[test]
    fn exec_macro_arg_substitution_in_shell() {
        let rs = parse_ok(
            r#"
define grab(limit):
    shell: |
        awk -v lim=$1 '{ if (NR<=lim) print }'

foo:
    grab 3
"#,
        );
        let out = execute(&rs, &ctx("foo", Level::Full), "a\nb\nc\nd\ne\n").unwrap();
        assert_eq!(out, "a\nb\nc\n");
    }

    #[test]
    fn pep723_detection() {
        assert!(has_pep723_header(
            "# /// script\n# dependencies = []\n# ///\nimport sys"
        ));
        assert!(has_pep723_header(
            "    # /// script\n    # ///\nimport sys"
        ));
        assert!(!has_pep723_header("import sys\nprint('hi')"));
        assert!(!has_pep723_header("# not pep 723\nprint('hi')"));
    }

    #[test]
    fn kubectl_compact_plugin_parses() {
        let src = include_str!(
            "../../../test-fixtures/plugins/kubectl/kubectl-compact/filter.lf"
        );
        let rs = parse_ok(src);
        // Define: clean-yaml (with PEP 723 body)
        assert_eq!(rs.defines.len(), 1);
        assert_eq!(rs.defines[0].name, "clean-yaml");
        match &rs.defines[0].ops[0] {
            Op::Python(body) => {
                assert!(body.contains("# /// script"));
                assert!(body.contains("dependencies = [\"pyyaml>=6\"]"));
                assert!(body.contains("yaml.safe_load_all"));
            }
            other => panic!("expected Python op, got {other:?}"),
        }
        // get/logs/events/* selection
        assert!(rs.select("get", Level::Full).is_some());
        assert!(rs.select("logs", Level::Ultra).is_some());
        assert!(rs.select("logs", Level::Full).is_some());
        assert!(rs.select("events", Level::Ultra).is_some());
        assert!(rs.select("describe", Level::Full).is_some()); // catch-all
    }

    // ── v2: cascades, guards, globs ───────────────────────────────

    #[test]
    fn parse_cascade_arms() {
        let rs = parse_ok(
            r#"
diff:
    if exit failed: raw
    elif level ultra: head 5
    else: head 99
"#,
        );
        match &rs.rules[0].ops[..] {
            [Op::Cascade(branches)] => {
                assert_eq!(branches.len(), 3);
                assert!(branches[0].guard.is_some());
                assert!(branches[1].guard.is_some());
                assert!(branches[2].guard.is_none());
            }
            other => panic!("expected one Cascade op, got {other:?}"),
        }
    }

    #[test]
    fn exec_cascade_branches_on_exit() {
        let rs = parse_ok(
            r#"
diff:
    if exit failed: raw
    else: head 1
"#,
        );
        let input = "a\nb\nc\n";
        let failed = ExecCtx { sub: "diff", level: Level::Full, exit_code: 1, args: &[] };
        let ok = ExecCtx { sub: "diff", level: Level::Full, exit_code: 0, args: &[] };
        assert_eq!(execute(&rs, &failed, input).unwrap(), "a\nb\nc\n");
        assert_eq!(execute(&rs, &ok, input).unwrap(), "a\n");
    }

    #[test]
    fn exec_cascade_level_and_flag_guards() {
        let rs = parse_ok(
            r#"
diff:
    if level ultra and --stat: head 1
    elif --stat: head 2
    else: head 3
"#,
        );
        let input = "1\n2\n3\n4\n";
        let stat = vec!["--stat".to_string()];
        let ultra_stat = ExecCtx { sub: "diff", level: Level::Ultra, exit_code: 0, args: &stat };
        let full_stat = ExecCtx { sub: "diff", level: Level::Full, exit_code: 0, args: &stat };
        let plain = ExecCtx { sub: "diff", level: Level::Full, exit_code: 0, args: &[] };
        assert_eq!(execute(&rs, &ultra_stat, input).unwrap(), "1\n");
        assert_eq!(execute(&rs, &full_stat, input).unwrap(), "1\n2\n");
        assert_eq!(execute(&rs, &plain, input).unwrap(), "1\n2\n3\n");
    }

    #[test]
    fn exec_cascade_no_match_no_else_passes_through() {
        let rs = parse_ok("diff:\n    if exit failed: head 1\n");
        let out = execute(&rs, &ctx("diff", Level::Full), "x\ny\n").unwrap();
        assert_eq!(out, "x\ny\n");
    }

    #[test]
    fn exec_raw_is_identity() {
        // `raw` is canonical; `passthrough` is a legacy alias for the same op.
        for kw in ["raw", "passthrough"] {
            let rs = parse_ok(&format!("diff:\n    {kw}\n"));
            let out = execute(&rs, &ctx("diff", Level::Full), "x\ny\n").unwrap();
            assert_eq!(out, "x\ny\n");
        }
    }

    #[test]
    fn glob_selector_matches_prefix() {
        let rs = parse_ok("apply*:\n    head 1\n");
        assert!(rs.select("apply", Level::Full).is_some());
        assert!(rs.select("apply-set", Level::Full).is_some());
        assert!(rs.select("delete", Level::Full).is_none());
    }

    #[test]
    fn or_is_alias_of_else() {
        let new = parse_ok("s:\n    keep /Z/\n    or \"clean\"\n");
        let old = parse_ok("s:\n    keep /Z/\n    else \"clean\"\n");
        assert_eq!(execute(&new, &ctx("s", Level::Full), "nope\n").unwrap(), "clean\n");
        assert_eq!(execute(&old, &ctx("s", Level::Full), "nope\n").unwrap(), "clean\n");
    }

    #[test]
    fn errors_on_unknown_guard_value() {
        let chain = format!("{:#}", parse("diff:\n    if exit boom: head 1\n").unwrap_err());
        assert!(chain.contains("unknown exit value"), "got: {chain}");
    }

    // ── match: single-dimension cascade sugar ─────────────────────

    #[test]
    fn parse_match_level_desugars_to_cascade() {
        let rs = parse_ok(
            r#"
state:
    match level:
        ultra: head 1
        lite:  head 3
        else:  head 2
"#,
        );
        match &rs.rules[0].ops[..] {
            [Op::Cascade(branches)] => {
                assert_eq!(branches.len(), 3);
                assert!(matches!(
                    branches[0].guard.as_ref().unwrap().atoms.as_slice(),
                    [Atom::Level(Level::Ultra)]
                ));
                assert!(matches!(
                    branches[1].guard.as_ref().unwrap().atoms.as_slice(),
                    [Atom::Level(Level::Lite)]
                ));
                assert!(branches[2].guard.is_none());
            }
            other => panic!("expected one Cascade op, got {other:?}"),
        }
    }

    #[test]
    fn exec_match_level_matches_equivalent_cascade() {
        let m = parse_ok(
            r#"
state:
    match level:
        ultra: head 1
        lite:  head 3
        else:  head 2
"#,
        );
        let c = parse_ok(
            r#"
state:
    if level ultra: head 1
    elif level lite: head 3
    else: head 2
"#,
        );
        let input = "a\nb\nc\nd\n";
        for level in [Level::Ultra, Level::Full, Level::Lite] {
            let mc = execute(&m, &ctx("state", level), input).unwrap();
            let cc = execute(&c, &ctx("state", level), input).unwrap();
            assert_eq!(mc, cc, "level {level:?}");
        }
    }

    #[test]
    fn exec_match_exit() {
        let rs = parse_ok(
            r#"
diff:
    match exit:
        failed: raw
        ok: head 1
"#,
        );
        let input = "a\nb\nc\n";
        let failed = ExecCtx { sub: "diff", level: Level::Full, exit_code: 1, args: &[] };
        let okctx = ExecCtx { sub: "diff", level: Level::Full, exit_code: 0, args: &[] };
        assert_eq!(execute(&rs, &failed, input).unwrap(), "a\nb\nc\n");
        assert_eq!(execute(&rs, &okctx, input).unwrap(), "a\n");
    }

    #[test]
    fn exec_nested_match_inside_else_arm() {
        let rs = parse_ok(
            r#"
plan:
    if exit failed:
        raw
    else:
        match level:
            ultra: head 1
            lite:  head 3
            else:  head 2
"#,
        );
        let input = "a\nb\nc\nd\n";
        let failed = ExecCtx { sub: "plan", level: Level::Full, exit_code: 1, args: &[] };
        let ok_full = ExecCtx { sub: "plan", level: Level::Full, exit_code: 0, args: &[] };
        let ok_ultra = ExecCtx { sub: "plan", level: Level::Ultra, exit_code: 0, args: &[] };
        let ok_lite = ExecCtx { sub: "plan", level: Level::Lite, exit_code: 0, args: &[] };
        assert_eq!(execute(&rs, &failed, input).unwrap(), input);
        assert_eq!(execute(&rs, &ok_full, input).unwrap(), "a\nb\n");
        assert_eq!(execute(&rs, &ok_ultra, input).unwrap(), "a\n");
        assert_eq!(execute(&rs, &ok_lite, input).unwrap(), "a\nb\nc\n");
    }

    #[test]
    fn match_missing_dimension_errors() {
        let chain = format!("{:#}", parse("plan:\n    match:\n        ultra: head 1\n").unwrap_err());
        assert!(chain.contains("needs a dimension"), "got: {chain}");
    }

    #[test]
    fn match_unknown_dimension_errors() {
        let chain = format!(
            "{:#}",
            parse("plan:\n    match flag:\n        x: head 1\n").unwrap_err()
        );
        assert!(chain.contains("unknown match dimension"), "got: {chain}");
    }

    #[test]
    fn match_unknown_value_errors() {
        let chain = format!(
            "{:#}",
            parse("plan:\n    match exit:\n        boom: head 1\n").unwrap_err()
        );
        assert!(chain.contains("unknown exit value"), "got: {chain}");
    }

    #[test]
    fn match_inline_after_header_errors() {
        let chain = format!(
            "{:#}",
            parse("plan:\n    match level: head 1\n").unwrap_err()
        );
        assert!(
            chain.contains("doesn't take inline ops"),
            "got: {chain}"
        );
    }
}
