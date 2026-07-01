//! Length expressions with units (SPEC §5.2–5.3).
//!
//! Canonical internal unit is millimetres. Supported length units: mm, cm, m, ft, in.
//! Bare numbers are interpreted as millimetres. Identifiers refer to document parameters.

use crate::model::Document;
use serde::{Deserialize, Serialize};

/// Evaluate a length expression to millimetres, or `None` if parsing fails.
pub fn eval_length_mm(text: &str) -> Option<f32> {
    eval_length_mm_with_params(text, &[])
}

/// Evaluate a length expression using document parameters.
pub fn eval_length_mm_in_doc(text: &str, doc: &Document) -> Option<f32> {
    let params: Vec<(&str, &str)> = doc
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p.expression.as_str()))
        .collect();
    eval_length_mm_with_params(text, &params)
}

/// Evaluate with explicit parameter name → expression bindings.
pub fn eval_length_mm_with_params(text: &str, params: &[(&str, &str)]) -> Option<f32> {
    let mut visiting = Vec::new();
    eval_length_mm_inner(text.trim(), params, &mut visiting)
}

fn eval_length_mm_inner(text: &str, params: &[(&str, &str)], visiting: &mut Vec<String>) -> Option<f32> {
    let mut p = Parser::new(text, Some(params), visiting);
    let value = p.parse_expr().ok()?;
    p.skip_ws();
    if p.at_end() {
        Some(value)
    } else {
        None
    }
}

/// Whether `name` matches a known length or angle unit suffix (case-insensitive).
pub fn parameter_name_conflicts_with_unit(name: &str) -> bool {
    let lower: String = name
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .collect();
    LENGTH_UNIT_SUFFIXES
        .iter()
        .chain(ANGLE_UNIT_SUFFIXES.iter())
        .any(|unit| lower == *unit)
}

/// Whether `name` is a valid parameter identifier.
pub fn is_valid_parameter_name(name: &str) -> bool {
    if name.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    if parameter_name_conflicts_with_unit(name) {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Replace whole identifier occurrences of `old` with `new` in an expression.
pub fn substitute_parameter_name(expression: &str, old: &str, new: &str) -> String {
    if old == new || old.is_empty() || !is_valid_parameter_name(old) {
        return expression.to_string();
    }
    let mut out = String::with_capacity(expression.len());
    let mut i = 0;
    while i < expression.len() {
        if let Some((ident, len)) = identifier_at(expression, i) {
            if ident == old {
                out.push_str(new);
            } else {
                out.push_str(ident);
            }
            i += len;
        } else {
            let ch = expression[i..].chars().next().expect("char");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

const LENGTH_UNIT_SUFFIXES: &[&str] = &["mm", "cm", "ft", "in", "m"];
const ANGLE_UNIT_SUFFIXES: &[&str] = &["deg", "rad"];

fn is_unit_suffix_at(expression: &str, unit_start: usize, ident: &str) -> bool {
    is_length_unit_suffix_at(expression, unit_start, ident)
        || is_angle_unit_suffix_at(expression, unit_start, ident)
}

fn is_length_unit_suffix_at(expression: &str, unit_start: usize, ident: &str) -> bool {
    unit_suffix_follows_quantity(expression, unit_start, ident, LENGTH_UNIT_SUFFIXES)
}

fn is_angle_unit_suffix_at(expression: &str, unit_start: usize, ident: &str) -> bool {
    unit_suffix_follows_quantity(expression, unit_start, ident, ANGLE_UNIT_SUFFIXES)
}

fn unit_suffix_follows_quantity(
    expression: &str,
    unit_start: usize,
    ident: &str,
    units: &[&str],
) -> bool {
    if !units.contains(&ident) {
        return false;
    }
    let before = expression[..unit_start].trim_end();
    before
        .chars()
        .last()
        .is_some_and(|c| c.is_ascii_digit() || c == '.')
}

/// Variable-like identifiers in `expression`, excluding unit suffixes attached to numbers.
pub fn identifiers_in_expression(expression: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut i = 0;
    while i < expression.len() {
        if let Some((ident, len)) = identifier_at(expression, i) {
            if !is_unit_suffix_at(expression, i, ident)
                && !names.iter().any(|n| n == ident)
            {
                names.push(ident.to_string());
            }
            i += len;
        } else {
            let step = expression[i..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
            i += step;
        }
    }
    names
}

/// Whole identifiers in `expression` that match `known_names`.
pub fn parameter_names_referenced_in_expression(expression: &str, known_names: &[&str]) -> Vec<String> {
    identifiers_in_expression(expression)
        .into_iter()
        .filter(|name| known_names.contains(&name.as_str()))
        .collect()
}

/// Identifiers in `expression` that are not present in `known_names`.
pub fn unknown_variables_in_expression(expression: &str, known_names: &[&str]) -> Vec<String> {
    identifiers_in_expression(expression)
        .into_iter()
        .filter(|name| !known_names.contains(&name.as_str()))
        .collect()
}

/// Unknown parameter references when defining `param_name` (`existing_index` is `None` for a new row).
pub fn unknown_variables_in_parameter_expression(
    expression: &str,
    doc: &Document,
    param_name: &str,
    existing_index: Option<usize>,
) -> Vec<String> {
    let known = document_parameter_names(doc);
    identifiers_in_expression(expression)
        .into_iter()
        .filter(|name| {
            if known.contains(&name.as_str()) {
                return false;
            }
            !(existing_index.is_none() && name == param_name)
        })
        .collect()
}

pub fn format_unknown_variable_error(name: &str) -> String {
    format!("Unknown variable: {name}")
}

pub fn document_parameter_names<'a>(doc: &'a Document) -> Vec<&'a str> {
    doc.parameters.iter().map(|p| p.name.as_str()).collect()
}

/// Whether `expression` contains a whole identifier referencing a document parameter.
pub fn expression_references_document_parameter(doc: &Document, expression: &str) -> bool {
    let mut i = 0;
    while i < expression.len() {
        if let Some((ident, len)) = identifier_at(expression, i) {
            if doc.parameters.iter().any(|p| p.name == ident) {
                return true;
            }
            i += len;
        } else {
            let step = expression[i..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
            i += step;
        }
    }
    false
}

fn identifier_at(text: &str, start: usize) -> Option<(&str, usize)> {
    let rest = &text[start..];
    let mut chars = rest.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let mut len = first.len_utf8();
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            break;
        }
        len += c.len_utf8();
    }
    Some((&text[start..start + len], len))
}

/// Evaluated length for display above a dimension field, using document parameters.
pub fn computed_length_in_doc(text: &str, doc: &Document) -> Option<f32> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    eval_length_mm_in_doc(t, doc).or_else(|| eval_length_mm(t))
}

/// Whether the text uses expression syntax (operators, parentheses, or units) and
/// should show a computed value above the input field.
pub fn shows_computed_length(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if t.contains(['+', '*', '/', '(', ')']) {
        return true;
    }
    // Binary minus (not a lone leading sign on a simple number).
    if t.chars().skip(1).any(|c| c == '-') {
        return true;
    }
    has_length_unit_suffix(t)
}

/// Whether to show a computed value above a dimension field in the document context.
pub fn shows_computed_length_in_doc(text: &str, doc: &Document) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if shows_computed_length(t) {
        return true;
    }
    if is_valid_parameter_name(t) {
        return eval_length_mm_in_doc(t, doc).is_some();
    }
    computed_length_in_doc(t, doc).is_some()
}

/// Format a length in millimetres for display above an expression field.
///
/// Kept hardcoded to mm intentionally: the Lua scripting numeric API is unit-agnostic-in-mm
/// by design, and `script.rs`'s tests assert on this literal raw-mm output. UI display call
/// sites should use [`format_length_display_in`] instead (#85), which is unit-parameterized;
/// this function is retained only for that scripting contract (hence `#[allow(dead_code)]`
/// outside `cfg(test)` builds, since production UI code no longer calls it directly).
#[allow(dead_code)]
pub fn format_length_display(v: f32) -> String {
    if v.abs() < 0.1 {
        "0 mm".to_string()
    } else {
        format!("{:.1} mm", v)
    }
}

/// Format a circle diameter for dimension labels (architectural naught prefix).
///
/// See [`format_length_display`]: kept hardcoded to mm for the scripting contract; UI call
/// sites should use [`format_diameter_display_in`] instead (#85).
#[allow(dead_code)]
pub fn format_diameter_display(v: f32) -> String {
    if v.abs() < 0.1 {
        "Ø0 mm".to_string()
    } else {
        format!("Ø{:.1} mm", v)
    }
}

/// Format a length (stored internally in mm) for display in `unit` (#85).
///
/// The near-zero snap threshold is checked in mm-space so it doesn't vary by unit.
pub fn format_length_display_in(v_mm: f32, unit: LengthUnit) -> String {
    if v_mm.abs() < 0.1 {
        format!("0 {}", unit.label())
    } else {
        format!("{:.1} {}", v_mm / unit.to_mm(), unit.label())
    }
}

/// Format a circle diameter (stored internally in mm) for display in `unit` (#85).
pub fn format_diameter_display_in(v_mm: f32, unit: LengthUnit) -> String {
    if v_mm.abs() < 0.1 {
        format!("Ø0 {}", unit.label())
    } else {
        format!("Ø{:.1} {}", v_mm / unit.to_mm(), unit.label())
    }
}

/// Parse a length expression, falling back when empty/invalid.
pub fn parse_length_or(text: &str, fallback: f32) -> f32 {
    eval_length_mm(text).unwrap_or(fallback)
}

/// Parse a length expression with parameters, falling back when empty/invalid.
pub fn parse_length_or_in_doc(text: &str, doc: &Document, fallback: f32) -> f32 {
    eval_length_mm_in_doc(text, doc)
        .unwrap_or(fallback)
}

/// Parse a positive length expression with parameters.
pub fn parse_positive_length_or_in_doc(text: &str, doc: &Document, fallback: f32) -> f32 {
    let v = parse_length_or_in_doc(text, doc, fallback);
    if v > 0.0 { v } else { fallback }
}

/// Evaluate an angle expression to radians, or `None` if parsing fails.
/// Bare numbers are interpreted as degrees; suffixes `deg` and `rad` are supported.
pub fn eval_angle_rad(text: &str) -> Option<f32> {
    eval_angle_rad_with_params(text, &[])
}

/// Evaluate an angle expression using document parameters.
pub fn eval_angle_rad_in_doc(text: &str, doc: &Document) -> Option<f32> {
    let params: Vec<(&str, &str)> = doc
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p.expression.as_str()))
        .collect();
    eval_angle_rad_with_params(text, &params)
}

pub fn eval_angle_rad_with_params(text: &str, params: &[(&str, &str)]) -> Option<f32> {
    let mut visiting = Vec::new();
    eval_angle_rad_inner(text.trim(), params, &mut visiting)
}

/// Evaluated parameter value in canonical internal units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EvaluatedParameter {
    LengthMm(f32),
    AngleRad(f32),
}

/// Evaluate a parameter expression to a length or angle value.
pub fn eval_parameter_in_doc(text: &str, doc: &Document) -> Option<EvaluatedParameter> {
    eval_length_mm_in_doc(text, doc)
        .map(EvaluatedParameter::LengthMm)
        .or_else(|| {
            eval_angle_rad_in_doc(text, doc).map(EvaluatedParameter::AngleRad)
        })
}

/// Whether a parameter expression parses as a length or angle value.
pub fn valid_parameter_expression_with_params(text: &str, params: &[(&str, &str)]) -> bool {
    eval_length_mm_with_params(text, params).is_some()
        || eval_angle_rad_with_params(text, params).is_some()
}

fn eval_angle_rad_inner(text: &str, params: &[(&str, &str)], visiting: &mut Vec<String>) -> Option<f32> {
    let mut p = AngleParser::new(text, Some(params), visiting);
    let value = p.parse_expr().ok()?;
    p.skip_ws();
    if p.at_end() {
        Some(value)
    } else {
        None
    }
}

/// Format an angle in radians for dimension labels (degrees by default).
///
/// See [`format_length_display`]: kept hardcoded to degrees for the scripting contract; UI
/// call sites should use [`format_angle_display_in`] instead (#85).
#[allow(dead_code)]
pub fn format_angle_display(rad: f32) -> String {
    let deg = rad.to_degrees();
    if deg.abs() < 0.05 {
        "0 deg".to_string()
    } else {
        format!("{deg:.1} deg", deg = deg)
    }
}

/// Format an angle (stored internally in radians) for display in `unit` (#85).
pub fn format_angle_display_in(rad: f32, unit: AngleUnit) -> String {
    match unit {
        AngleUnit::Deg => {
            let deg = rad.to_degrees();
            if deg.abs() < 0.05 {
                "0 deg".to_string()
            } else {
                format!("{deg:.1} deg")
            }
        }
        AngleUnit::Rad => {
            if rad.abs() < 0.001 {
                "0 rad".to_string()
            } else {
                format!("{rad:.2} rad")
            }
        }
    }
}

/// Evaluated angle for display above a dimension field.
pub fn computed_angle_in_doc(text: &str, doc: &Document) -> Option<f32> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    eval_angle_rad_in_doc(t, doc).or_else(|| eval_angle_rad(t))
}

/// Whether to show a computed value above an angle dimension field.
pub fn shows_computed_angle(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if t.contains(['+', '*', '/', '(', ')']) {
        return true;
    }
    if t.chars().skip(1).any(|c| c == '-') {
        return true;
    }
    has_angle_unit_suffix(t)
}

pub fn shows_computed_angle_in_doc(text: &str, doc: &Document) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if shows_computed_angle(t) {
        return true;
    }
    if is_valid_parameter_name(t) {
        return eval_angle_rad_in_doc(t, doc).is_some();
    }
    computed_angle_in_doc(t, doc).is_some()
}

pub fn has_angle_unit_suffix(text: &str) -> bool {
    let lower: String = text
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .collect();
    ANGLE_UNIT_SUFFIXES.iter().any(|unit| {
        lower.ends_with(unit)
            && lower
                .strip_suffix(unit)
                .is_some_and(|prefix| prefix.chars().last().is_some_and(|c| c.is_ascii_digit()))
    })
}

fn has_length_unit_suffix(text: &str) -> bool {
    const UNITS: &[&str] = &["mm", "cm", "ft", "in", "m"];
    let lower: String = text
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .collect();
    UNITS.iter().any(|unit| {
        lower.ends_with(unit)
            && lower
                .strip_suffix(unit)
                .is_some_and(|prefix| prefix.chars().last().is_some_and(|c| c.is_ascii_digit()))
    })
}

/// A unit of length, as accepted by expression parsing and offered as a document/sketch
/// default in the context pane (#52).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LengthUnit {
    Mm,
    Cm,
    M,
    Ft,
    In,
}

impl Default for LengthUnit {
    /// Matches today's implicit parser fallback for bare numbers (mm).
    fn default() -> Self {
        LengthUnit::Mm
    }
}

impl LengthUnit {
    /// All variants, in the order they should be offered in a unit picker.
    pub const ALL: [LengthUnit; 5] = [
        LengthUnit::Mm,
        LengthUnit::Cm,
        LengthUnit::M,
        LengthUnit::Ft,
        LengthUnit::In,
    ];

    pub fn to_mm(self) -> f32 {
        match self {
            LengthUnit::Mm => 1.0,
            LengthUnit::Cm => 10.0,
            LengthUnit::M => 1000.0,
            LengthUnit::Ft => 304.8,
            LengthUnit::In => 25.4,
        }
    }

    /// Short label for UI pickers (e.g. `"mm"`).
    pub fn label(self) -> &'static str {
        match self {
            LengthUnit::Mm => "mm",
            LengthUnit::Cm => "cm",
            LengthUnit::M => "m",
            LengthUnit::Ft => "ft",
            LengthUnit::In => "in",
        }
    }

    /// Name used in Lua scripts (`bearcad.set_units{ length = "mm" }`).
    pub fn script_name(self) -> &'static str {
        self.label()
    }

    /// Parse a script/UI name back into a unit (case-insensitive); `None` if unrecognised.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "mm" => Some(LengthUnit::Mm),
            "cm" => Some(LengthUnit::Cm),
            "m" => Some(LengthUnit::M),
            "ft" => Some(LengthUnit::Ft),
            "in" => Some(LengthUnit::In),
            _ => None,
        }
    }
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    params: Option<&'a [(&'a str, &'a str)]>,
    visiting: &'a mut Vec<String>,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str, params: Option<&'a [(&'a str, &'a str)]>, visiting: &'a mut Vec<String>) -> Self {
        Self {
            chars: input.chars().peekable(),
            params,
            visiting,
        }
    }

    fn at_end(&mut self) -> bool {
        self.skip_ws();
        self.chars.peek().is_none()
    }

    fn skip_ws(&mut self) {
        while matches!(self.chars.peek(), Some(' ' | '\t')) {
            self.chars.next();
        }
    }

    fn bump(&mut self) -> Option<char> {
        self.chars.next()
    }

    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn parse_expr(&mut self) -> Result<f32, ()> {
        self.parse_add_sub()
    }

    fn parse_add_sub(&mut self) -> Result<f32, ()> {
        let mut acc = self.parse_mul_div()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('+') => {
                    self.bump();
                    acc += self.parse_mul_div()?;
                }
                Some('-') => {
                    self.bump();
                    acc -= self.parse_mul_div()?;
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    fn parse_mul_div(&mut self) -> Result<f32, ()> {
        let mut acc = self.parse_unary()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('*') => {
                    self.bump();
                    acc *= self.parse_unary()?;
                }
                Some('/') => {
                    self.bump();
                    let rhs = self.parse_unary()?;
                    if rhs.abs() < f32::EPSILON {
                        return Err(());
                    }
                    acc /= rhs;
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    fn parse_unary(&mut self) -> Result<f32, ()> {
        self.skip_ws();
        match self.peek() {
            Some('-') => {
                self.bump();
                Ok(-self.parse_unary()?)
            }
            Some('+') => {
                self.bump();
                self.parse_unary()
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<f32, ()> {
        self.skip_ws();
        if self.peek() == Some('(') {
            self.bump();
            let v = self.parse_expr()?;
            self.skip_ws();
            if self.peek() != Some(')') {
                return Err(());
            }
            self.bump();
            return Ok(v);
        }
        if let Some(name) = self.try_parse_identifier() {
            return self.resolve_identifier(name);
        }
        self.parse_quantity()
    }

    fn try_parse_identifier(&mut self) -> Option<String> {
        self.skip_ws();
        let rest: String = self.chars.clone().collect();
        let (ident, len) = identifier_at(&rest, 0)?;
        for _ in 0..len {
            self.bump();
        }
        Some(ident.to_string())
    }

    fn resolve_identifier(&mut self, name: String) -> Result<f32, ()> {
        let Some(params) = self.params else {
            return Err(());
        };
        if self.visiting.iter().any(|v| v == &name) {
            return Err(());
        }
        let expression = params
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, expr)| *expr)
            .ok_or(())?;
        self.visiting.push(name);
        let value = eval_length_mm_inner(expression, params, self.visiting).ok_or(())?;
        self.visiting.pop();
        Ok(value)
    }

    fn parse_quantity(&mut self) -> Result<f32, ()> {
        self.skip_ws();
        let n = self.parse_number()?;
        let unit = self.parse_unit()?;
        Ok(n * unit.to_mm())
    }

    fn parse_number(&mut self) -> Result<f32, ()> {
        self.skip_ws();
        let mut s = String::new();
        let mut saw_digit = false;
        let mut saw_dot = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                saw_digit = true;
                s.push(c);
                self.bump();
            } else if c == '.' && !saw_dot {
                saw_dot = true;
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        if !saw_digit {
            return Err(());
        }
        s.parse::<f32>().map_err(|_| ())
    }

    fn parse_unit(&mut self) -> Result<LengthUnit, ()> {
        self.skip_ws();
        let rest: String = self.chars.clone().collect();
        let lower: String = rest
            .chars()
            .map(|c| c.to_ascii_lowercase())
            .collect();
        for (suffix, unit, len) in [
            ("mm", LengthUnit::Mm, 2),
            ("cm", LengthUnit::Cm, 2),
            ("ft", LengthUnit::Ft, 2),
            ("in", LengthUnit::In, 2),
            ("m", LengthUnit::M, 1),
        ] {
            if lower.starts_with(suffix) {
                let next = lower.as_bytes().get(len).copied();
                if next.is_none_or(|b| !b.is_ascii_alphabetic()) {
                    for _ in 0..len {
                        self.bump();
                    }
                    return Ok(unit);
                }
            }
        }
        Ok(LengthUnit::Mm)
    }
}

/// A unit of angle, as accepted by expression parsing and offered as a document/sketch
/// default in the context pane (#52).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AngleUnit {
    Deg,
    Rad,
}

impl Default for AngleUnit {
    /// Matches today's implicit parser fallback for bare numbers (degrees).
    fn default() -> Self {
        AngleUnit::Deg
    }
}

impl AngleUnit {
    /// All variants, in the order they should be offered in a unit picker.
    pub const ALL: [AngleUnit; 2] = [AngleUnit::Deg, AngleUnit::Rad];

    pub fn to_rad(self) -> f32 {
        match self {
            AngleUnit::Deg => std::f32::consts::PI / 180.0,
            AngleUnit::Rad => 1.0,
        }
    }

    /// Short label for UI pickers (e.g. `"deg"`).
    pub fn label(self) -> &'static str {
        match self {
            AngleUnit::Deg => "deg",
            AngleUnit::Rad => "rad",
        }
    }

    /// Name used in Lua scripts (`bearcad.set_units{ angle = "deg" }`).
    pub fn script_name(self) -> &'static str {
        self.label()
    }

    /// Parse a script/UI name back into a unit (case-insensitive); `None` if unrecognised.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "deg" => Some(AngleUnit::Deg),
            "rad" => Some(AngleUnit::Rad),
            _ => None,
        }
    }
}

struct AngleParser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    params: Option<&'a [(&'a str, &'a str)]>,
    visiting: &'a mut Vec<String>,
}

impl<'a> AngleParser<'a> {
    fn new(input: &'a str, params: Option<&'a [(&'a str, &'a str)]>, visiting: &'a mut Vec<String>) -> Self {
        Self {
            chars: input.chars().peekable(),
            params,
            visiting,
        }
    }

    fn at_end(&mut self) -> bool {
        self.skip_ws();
        self.chars.peek().is_none()
    }

    fn skip_ws(&mut self) {
        while matches!(self.chars.peek(), Some(' ' | '\t')) {
            self.chars.next();
        }
    }

    fn bump(&mut self) -> Option<char> {
        self.chars.next()
    }

    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn parse_expr(&mut self) -> Result<f32, ()> {
        self.parse_add_sub()
    }

    fn parse_add_sub(&mut self) -> Result<f32, ()> {
        let mut acc = self.parse_mul_div()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('+') => {
                    self.bump();
                    acc += self.parse_mul_div()?;
                }
                Some('-') => {
                    self.bump();
                    acc -= self.parse_mul_div()?;
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    fn parse_mul_div(&mut self) -> Result<f32, ()> {
        let mut acc = self.parse_unary()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('*') => {
                    self.bump();
                    acc *= self.parse_unary()?;
                }
                Some('/') => {
                    self.bump();
                    let rhs = self.parse_unary()?;
                    if rhs.abs() < f32::EPSILON {
                        return Err(());
                    }
                    acc /= rhs;
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    fn parse_unary(&mut self) -> Result<f32, ()> {
        self.skip_ws();
        match self.peek() {
            Some('-') => {
                self.bump();
                Ok(-self.parse_unary()?)
            }
            Some('+') => {
                self.bump();
                self.parse_unary()
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<f32, ()> {
        self.skip_ws();
        if self.peek() == Some('(') {
            self.bump();
            let v = self.parse_expr()?;
            self.skip_ws();
            if self.peek() != Some(')') {
                return Err(());
            }
            self.bump();
            return Ok(v);
        }
        if let Some(name) = self.try_parse_identifier() {
            return self.resolve_identifier(name);
        }
        self.parse_quantity()
    }

    fn try_parse_identifier(&mut self) -> Option<String> {
        self.skip_ws();
        let rest: String = self.chars.clone().collect();
        let (ident, len) = identifier_at(&rest, 0)?;
        for _ in 0..len {
            self.bump();
        }
        Some(ident.to_string())
    }

    fn resolve_identifier(&mut self, name: String) -> Result<f32, ()> {
        let Some(params) = self.params else {
            return Err(());
        };
        if self.visiting.iter().any(|v| v == &name) {
            return Err(());
        }
        let expression = params
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, expr)| *expr)
            .ok_or(())?;
        self.visiting.push(name);
        let value = eval_angle_rad_inner(expression, params, self.visiting).ok_or(())?;
        self.visiting.pop();
        Ok(value)
    }

    fn parse_quantity(&mut self) -> Result<f32, ()> {
        self.skip_ws();
        let n = self.parse_number()?;
        let unit = self.parse_unit()?;
        Ok(n * unit.to_rad())
    }

    fn parse_number(&mut self) -> Result<f32, ()> {
        self.skip_ws();
        let mut s = String::new();
        let mut saw_digit = false;
        let mut saw_dot = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                saw_digit = true;
                s.push(c);
                self.bump();
            } else if c == '.' && !saw_dot {
                saw_dot = true;
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        if !saw_digit {
            return Err(());
        }
        s.parse::<f32>().map_err(|_| ())
    }

    fn parse_unit(&mut self) -> Result<AngleUnit, ()> {
        self.skip_ws();
        let rest: String = self.chars.clone().collect();
        let lower: String = rest
            .chars()
            .map(|c| c.to_ascii_lowercase())
            .collect();
        for (suffix, unit, len) in [("deg", AngleUnit::Deg, 3), ("rad", AngleUnit::Rad, 3)] {
            if lower.starts_with(suffix) {
                let next = lower.as_bytes().get(len).copied();
                if next.is_none_or(|b| !b.is_ascii_alphabetic()) {
                    for _ in 0..len {
                        self.bump();
                    }
                    return Ok(unit);
                }
            }
        }
        Ok(AngleUnit::Deg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_number_is_mm() {
        assert!((eval_length_mm("10").unwrap() - 10.0).abs() < 1e-4);
        assert!((eval_length_mm("  3.5  ").unwrap() - 3.5).abs() < 1e-4);
    }

    #[test]
    fn unit_conversions() {
        assert!((eval_length_mm("1cm").unwrap() - 10.0).abs() < 1e-4);
        assert!((eval_length_mm("1m").unwrap() - 1000.0).abs() < 1e-4);
        assert!((eval_length_mm("1ft").unwrap() - 304.8).abs() < 1e-4);
        assert!((eval_length_mm("1in").unwrap() - 25.4).abs() < 1e-4);
        assert!((eval_length_mm("2 in").unwrap() - 50.8).abs() < 1e-4);
    }

    #[test]
    fn mixed_expression() {
        let v = eval_length_mm("2in + 5mm / 2").unwrap();
        assert!((v - 53.3).abs() < 1e-3, "got {v}");
    }

    #[test]
    fn arithmetic_precedence() {
        assert!((eval_length_mm("2 + 3 * 4").unwrap() - 14.0).abs() < 1e-4);
        assert!((eval_length_mm("(2 + 3) * 4").unwrap() - 20.0).abs() < 1e-4);
    }

    #[test]
    fn signed_lengths() {
        assert!((eval_length_mm("-5mm").unwrap() + 5.0).abs() < 1e-4);
        assert!((eval_length_mm("10mm - 15mm").unwrap() + 5.0).abs() < 1e-4);
    }

    #[test]
    fn invalid_expressions_return_none() {
        assert!(eval_length_mm("").is_none());
        assert!(eval_length_mm("abc").is_none());
        assert!(eval_length_mm("12x").is_none());
        assert!(eval_length_mm("2in +").is_none());
    }

    #[test]
    fn bare_angle_number_is_degrees() {
        assert!((eval_angle_rad("90").unwrap() - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        assert!((eval_angle_rad("45deg").unwrap() - std::f32::consts::FRAC_PI_4).abs() < 1e-4);
        assert!((eval_angle_rad("1.5708rad").unwrap() - 1.5708).abs() < 1e-4);
    }

    #[test]
    fn angle_expression_arithmetic() {
        let v = eval_angle_rad("45deg + 45").unwrap();
        assert!((v - std::f32::consts::FRAC_PI_2).abs() < 1e-3, "got {v}");
    }

    #[test]
    fn invalid_angle_expressions_return_none() {
        assert!(eval_angle_rad("").is_none());
        assert!(eval_angle_rad("abc").is_none());
        assert!(eval_angle_rad("45 +").is_none());
    }

    #[test]
    fn shows_computed_length_detects_syntax() {
        assert!(!shows_computed_length(""));
        assert!(!shows_computed_length("50"));
        assert!(!shows_computed_length("50.0"));
        assert!(shows_computed_length("2in"));
        assert!(shows_computed_length("2in + 5mm / 2"));
        assert!(shows_computed_length("(10 + 5)mm"));
        assert!(shows_computed_length("10 - 5"));
    }

    #[test]
    fn parse_positive_length_or_rejects_non_positive() {
        let doc = Document::default();
        assert!((parse_positive_length_or_in_doc("0", &doc, 9.0) - 9.0).abs() < 1e-4);
        assert!((parse_positive_length_or_in_doc("-3", &doc, 9.0) - 9.0).abs() < 1e-4);
        assert!((parse_positive_length_or_in_doc("2in", &doc, 9.0) - 50.8).abs() < 1e-3);
    }

    #[test]
    fn format_diameter_display_uses_naught_prefix() {
        assert_eq!(format_diameter_display(0.0), "Ø0 mm");
        assert_eq!(format_diameter_display(53.3), "Ø53.3 mm");
    }

    #[test]
    fn format_length_display_includes_mm_unit() {
        assert_eq!(format_length_display(0.0), "0 mm");
        assert_eq!(format_length_display(53.3), "53.3 mm");
    }

    #[test]
    fn format_length_display_in_converts_to_target_unit() {
        assert_eq!(format_length_display_in(0.0, LengthUnit::In), "0 in");
        assert_eq!(format_length_display_in(25.4, LengthUnit::In), "1.0 in");
        assert_eq!(format_length_display_in(304.8, LengthUnit::Ft), "1.0 ft");
        assert_eq!(format_length_display_in(1000.0, LengthUnit::M), "1.0 m");
        assert_eq!(format_length_display_in(10.0, LengthUnit::Cm), "1.0 cm");
        assert_eq!(format_length_display_in(53.3, LengthUnit::Mm), "53.3 mm");
        // Zero-snap threshold stays in mm-space, not converted-unit space.
        assert_eq!(format_length_display_in(0.05, LengthUnit::In), "0 in");
    }

    #[test]
    fn format_diameter_display_in_converts_to_target_unit() {
        assert_eq!(format_diameter_display_in(0.0, LengthUnit::In), "Ø0 in");
        assert_eq!(format_diameter_display_in(25.4, LengthUnit::In), "Ø1.0 in");
        assert_eq!(format_diameter_display_in(53.3, LengthUnit::Mm), "Ø53.3 mm");
    }

    #[test]
    fn format_angle_display_in_supports_deg_and_rad() {
        assert_eq!(format_angle_display_in(0.0, AngleUnit::Deg), "0 deg");
        assert_eq!(
            format_angle_display_in(std::f32::consts::FRAC_PI_2, AngleUnit::Deg),
            "90.0 deg"
        );
        assert_eq!(format_angle_display_in(0.0, AngleUnit::Rad), "0 rad");
        assert_eq!(
            format_angle_display_in(std::f32::consts::PI, AngleUnit::Rad),
            "3.14 rad"
        );
    }

    #[test]
    fn expression_string_round_trips_via_eval() {
        let expr = "2in + 5mm / 2";
        let v = eval_length_mm(expr).unwrap();
        assert!((v - 53.3).abs() < 1e-3);
        // Stored text is preserved by callers; re-evaluating yields the same value.
        assert!((eval_length_mm(expr).unwrap() - v).abs() < 1e-6);
    }

    #[test]
    fn shows_computed_length_in_doc_for_parameter_name() {
        let mut doc = Document::default();
        doc.parameters.push(crate::model::Parameter {
            name: "A".to_string(),
            expression: "10mm".to_string(),
            deleted: false,
            source: None,
        });
        assert!(shows_computed_length_in_doc("A", &doc));
        assert_eq!(computed_length_in_doc("A", &doc), Some(10.0));
    }

    #[test]
    fn eval_with_parameter_references() {
        let params = [("A", "5mm"), ("B", "A + 5in")];
        let v = eval_length_mm_with_params("B", &params).unwrap();
        assert!((v - (5.0 + 5.0 * 25.4)).abs() < 1e-2, "got {v}");
    }

    #[test]
    fn eval_detects_parameter_cycles() {
        let params = [("A", "B"), ("B", "A")];
        assert!(eval_length_mm_with_params("A", &params).is_none());
    }

    #[test]
    fn parameter_names_referenced_in_expression_finds_known_names() {
        let known = ["A", "B", "width"];
        assert_eq!(
            parameter_names_referenced_in_expression("A + width + A2", &known),
            vec!["A".to_string(), "width".to_string()]
        );
        assert!(parameter_names_referenced_in_expression("10mm", &known).is_empty());
    }

    #[test]
    fn identifiers_in_expression_ignores_unit_suffixes() {
        assert!(identifiers_in_expression("10mm").is_empty());
        assert!(identifiers_in_expression("2in + 5mm").is_empty());
        assert!(identifiers_in_expression("45deg").is_empty());
        assert!(identifiers_in_expression("1.57rad + 5deg").is_empty());
        assert_eq!(identifiers_in_expression("A + 2in"), vec!["A".to_string()]);
    }

    #[test]
    fn eval_angle_with_angle_parameter_references() {
        let params = [("corner", "45deg"), ("total", "corner + 5deg")];
        let v = eval_angle_rad_with_params("total", &params).unwrap();
        assert!((v.to_degrees() - 50.0).abs() < 1e-3, "got {}", v.to_degrees());
    }

    #[test]
    fn eval_parameter_in_doc_accepts_length_or_angle() {
        let mut doc = Document::default();
        doc.parameters.push(crate::model::Parameter {
            name: "width".to_string(),
            expression: "10mm".to_string(),
            deleted: false,
            source: None,
        });
        doc.parameters.push(crate::model::Parameter {
            name: "corner".to_string(),
            expression: "45deg".to_string(),
            deleted: false,
            source: None,
        });
        assert_eq!(
            eval_parameter_in_doc("width", &doc),
            Some(EvaluatedParameter::LengthMm(10.0))
        );
        let angle = eval_parameter_in_doc("corner", &doc).unwrap();
        match angle {
            EvaluatedParameter::AngleRad(v) => {
                assert!((v.to_degrees() - 45.0).abs() < 1e-3);
            }
            _ => panic!("expected angle parameter"),
        }
    }

    #[test]
    fn unknown_variables_in_expression_lists_missing_names() {
        let known = ["A"];
        assert_eq!(
            unknown_variables_in_expression("A + B", &known),
            vec!["B".to_string()]
        );
        assert!(unknown_variables_in_expression("10mm", &known).is_empty());
    }

    #[test]
    fn substitute_parameter_name_preserves_other_identifiers() {
        let expr = "A + width + A2";
        assert_eq!(substitute_parameter_name(expr, "A", "Len"), "Len + width + A2");
    }

    #[test]
    fn is_valid_parameter_name_rules() {
        assert!(is_valid_parameter_name("A"));
        assert!(is_valid_parameter_name("width_1"));
        assert!(!is_valid_parameter_name("1width"));
        assert!(!is_valid_parameter_name(""));
        assert!(!is_valid_parameter_name("my width"));
        assert!(!is_valid_parameter_name("width "));
    }

    #[test]
    fn parameter_name_conflicts_with_known_units() {
        for unit in ["mm", "cm", "m", "ft", "in", "deg", "rad"] {
            assert!(parameter_name_conflicts_with_unit(unit));
            let upper = unit.to_ascii_uppercase();
            assert!(
                parameter_name_conflicts_with_unit(&upper),
                "expected conflict for {upper}"
            );
            let mixed = format!("{}{}", &unit[..1].to_ascii_uppercase(), &unit[1..]);
            assert!(
                parameter_name_conflicts_with_unit(&mixed),
                "expected conflict for {mixed}"
            );
            assert!(!is_valid_parameter_name(unit));
        }
        assert!(!parameter_name_conflicts_with_unit("width"));
        assert!(is_valid_parameter_name("width"));
    }

    #[test]
    fn length_unit_defaults_to_mm_matching_bare_number_fallback() {
        assert_eq!(LengthUnit::default(), LengthUnit::Mm);
    }

    #[test]
    fn angle_unit_defaults_to_deg_matching_bare_number_fallback() {
        assert_eq!(AngleUnit::default(), AngleUnit::Deg);
    }

    #[test]
    fn length_unit_name_round_trips_through_script_name() {
        for unit in LengthUnit::ALL {
            assert_eq!(LengthUnit::from_name(unit.script_name()), Some(unit));
        }
        assert_eq!(LengthUnit::from_name("MM"), Some(LengthUnit::Mm));
        assert_eq!(LengthUnit::from_name("furlongs"), None);
    }

    #[test]
    fn angle_unit_name_round_trips_through_script_name() {
        for unit in AngleUnit::ALL {
            assert_eq!(AngleUnit::from_name(unit.script_name()), Some(unit));
        }
        assert_eq!(AngleUnit::from_name("DEG"), Some(AngleUnit::Deg));
        assert_eq!(AngleUnit::from_name("gradians"), None);
    }
}