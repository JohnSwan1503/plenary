//! Deliberately tiny hand parser for
//!
//! ```text
//! SELECT <col, col, ... | *> FROM <table> [WHERE <anything>]
//! ```
//!
//! Ported from the stq carrier prototype. `sqlparser` would be the real
//! choice; this keeps the example transparent and dependency-free.
//!
//! Column references are gathered from the select list and, in the WHERE
//! clause, from any bare identifier that is immediately followed by a
//! comparison operator (`org_id = {org_id}` → `org_id`). `{name}` is a
//! parameter placeholder, not a column. A `column <op> {param}` sequence is
//! also recorded as a comparison, so the collection can require the two
//! types to agree.

#[derive(Debug)]
pub struct ParsedSql {
    pub select: Select,
    pub from: String,
    pub where_clause: Option<String>,
    /// Column names referenced in WHERE (left-hand side of comparisons).
    pub where_columns: Vec<String>,
    /// `{param}` placeholders, in order of appearance.
    pub params: Vec<String>,
    /// `(column, param)` for every `column <op> {param}` in WHERE.
    pub comparisons: Vec<(String, String)>,
}

#[derive(Debug)]
pub enum Select {
    Star,
    Columns(Vec<String>),
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_op(s: &str) -> bool {
    matches!(s, "=" | "<" | ">" | "<=" | ">=" | "<>" | "!=" | "LIKE" | "like")
}

pub fn parse(sql: &str) -> Result<ParsedSql, String> {
    let words: Vec<&str> = sql.split_whitespace().collect();
    if words.first().map(|w| w.eq_ignore_ascii_case("select")) != Some(true) {
        return Err("stq-lite only understands `SELECT ... FROM ... [WHERE ...]`".into());
    }
    let from_idx = words
        .iter()
        .position(|w| w.eq_ignore_ascii_case("from"))
        .ok_or("missing FROM")?;
    let select_src = words[1..from_idx].join(" ");
    let select = if select_src.trim() == "*" {
        Select::Star
    } else {
        let cols: Vec<String> = select_src
            .split(',')
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        if cols.is_empty() {
            return Err("empty select list".into());
        }
        for c in &cols {
            if !is_ident(c) {
                return Err(format!("`{c}` is not a plain column name (stq-lite limitation)"));
            }
        }
        Select::Columns(cols)
    };
    let table = words
        .get(from_idx + 1)
        .ok_or("missing table name after FROM")?
        .to_string();
    if !is_ident(&table) {
        return Err(format!("`{table}` is not a plain table name"));
    }

    let mut where_clause = None;
    let mut where_columns = Vec::new();
    let mut params = Vec::new();
    let mut comparisons = Vec::new();
    if let Some(rest) = words.get(from_idx + 2..) {
        if !rest.is_empty() {
            if !rest[0].eq_ignore_ascii_case("where") {
                return Err(format!("expected WHERE or end of query, found `{}`", rest[0]));
            }
            let w = rest[1..].join(" ");
            let toks: Vec<&str> = w.split_whitespace().collect();
            for (i, t) in toks.iter().enumerate() {
                if let Some(inner) = t.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
                    params.push(inner.to_string());
                } else if is_ident(t)
                    && !t.eq_ignore_ascii_case("and")
                    && !t.eq_ignore_ascii_case("or")
                    && toks.get(i + 1).map(|op| is_op(op)).unwrap_or(false)
                {
                    where_columns.push(t.to_string());
                    if let Some(p) = toks
                        .get(i + 2)
                        .and_then(|p| p.strip_prefix('{'))
                        .and_then(|p| p.strip_suffix('}'))
                    {
                        comparisons.push((t.to_string(), p.to_string()));
                    }
                }
            }
            where_clause = Some(w);
        }
    }

    Ok(ParsedSql { select, from: table, where_clause, where_columns, params, comparisons })
}

impl ParsedSql {
    pub fn referenced_columns(&self) -> Vec<String> {
        let mut out = match &self.select {
            Select::Star => Vec::new(),
            Select::Columns(c) => c.clone(),
        };
        out.extend(self.where_columns.iter().cloned());
        out
    }

    /// Render the final SQL. `SELECT *` becomes the explicit column list and
    /// `{param}` placeholders become positional `$1, $2, ...`.
    pub fn render(&self, table_columns: &[String]) -> String {
        let cols = match &self.select {
            Select::Star => table_columns.join(", "),
            Select::Columns(c) => c.join(", "),
        };
        let mut s = format!("SELECT {cols} FROM {}", self.from);
        if let Some(w) = &self.where_clause {
            let mut n = 0;
            let mut rendered = String::new();
            for (i, t) in w.split_whitespace().enumerate() {
                if i > 0 {
                    rendered.push(' ');
                }
                if t.starts_with('{') && t.ends_with('}') {
                    n += 1;
                    rendered.push_str(&format!("${n}"));
                } else {
                    rendered.push_str(t);
                }
            }
            s.push_str(" WHERE ");
            s.push_str(&rendered);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparisons_are_paired() {
        let p = parse("SELECT id FROM users WHERE org_id = {org} AND email LIKE {pat}").unwrap();
        assert_eq!(p.comparisons, vec![("org_id".into(), "org".into()), ("email".into(), "pat".into())]);
        assert_eq!(p.render(&[]), "SELECT id FROM users WHERE org_id = $1 AND email LIKE $2");
    }
}
