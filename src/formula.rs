//! Sandboxed, deterministic formula expressions.
//!
//! Formula source uses a deliberately small JavaScript expression subset. Oxc
//! performs syntax parsing, while this module evaluates the resulting ESTree in
//! Rust. No JavaScript VM, host functions, filesystem access, or ambient state
//! are exposed.

use crate::schema::FormulaResultType;
use oxc_allocator::Allocator;
use oxc_ast::ast::Statement;
use oxc_parser::Parser;
use oxc_span::SourceType;
use regex::{Regex, RegexBuilder};
use rust_decimal::prelude::{MathematicalOps, ToPrimitive};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::str::FromStr;

pub const FORMULA_MODULE_ID: &str = "formula";

const RESERVED_FORMULA_NAMES: &[&str] = &["title", "path"];
const FORBIDDEN_IDENTIFIERS: &[&str] = &[
    "eval",
    "Function",
    "AsyncFunction",
    "GeneratorFunction",
    "WebAssembly",
    "globalThis",
    "window",
    "process",
    "require",
];
const FORBIDDEN_PROPERTIES: &[&str] = &["__proto__", "prototype", "constructor"];
const CALLBACK_METHODS: &[&str] = &["map", "filter", "reduce", "some", "every", "find"];
const BUILTIN_ROOTS: &[&str] = &[
    "Math",
    "Number",
    "String",
    "Array",
    "Object",
    "JSON",
    "Date",
    "parseInt",
    "parseFloat",
    "isFinite",
    "isInteger",
    "encodeURIComponent",
    "decodeURIComponent",
    "undefined",
];

fn is_allowed_global_call(name: &str) -> bool {
    matches!(
        name,
        "Number"
            | "String"
            | "parseInt"
            | "parseFloat"
            | "isFinite"
            | "isInteger"
            | "encodeURIComponent"
            | "decodeURIComponent"
    )
}

fn is_allowed_static_call(root: &str, method: &str) -> bool {
    match root {
        "Math" => matches!(
            method,
            "abs"
                | "ceil"
                | "floor"
                | "round"
                | "trunc"
                | "min"
                | "max"
                | "pow"
                | "sqrt"
                | "sign"
                | "exp"
                | "log"
                | "log10"
        ),
        "Number" => matches!(method, "isFinite" | "isInteger" | "parseInt" | "parseFloat"),
        "Array" => method == "isArray",
        "Object" => matches!(method, "keys" | "values" | "entries" | "hasOwn"),
        "JSON" => matches!(method, "parse" | "stringify"),
        "Date" => matches!(method, "parse" | "UTC"),
        "String" => false,
        _ => false,
    }
}

fn is_allowed_instance_call(method: &str) -> bool {
    matches!(
        method,
        "trim"
            | "trimStart"
            | "trimEnd"
            | "toUpperCase"
            | "toLowerCase"
            | "includes"
            | "startsWith"
            | "endsWith"
            | "charAt"
            | "at"
            | "slice"
            | "substring"
            | "indexOf"
            | "lastIndexOf"
            | "concat"
            | "repeat"
            | "padStart"
            | "padEnd"
            | "split"
            | "match"
            | "replace"
            | "join"
            | "flat"
            | "map"
            | "filter"
            | "reduce"
            | "some"
            | "every"
            | "find"
            | "test"
            | "toString"
            | "toFixed"
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormulaSourceSpan {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormulaDiagnostic {
    pub module: String,
    pub field: String,
    pub code: String,
    pub message: String,
    pub span: Option<FormulaSourceSpan>,
}

impl FormulaDiagnostic {
    fn new(
        field: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        span: Option<FormulaSourceSpan>,
    ) -> Self {
        Self {
            module: FORMULA_MODULE_ID.to_owned(),
            field: field.into(),
            code: code.into(),
            message: message.into(),
            span,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormulaDefinition {
    pub name: String,
    pub formula: String,
    pub result_type: FormulaResultType,
}

impl FormulaDefinition {
    pub fn new(
        name: impl Into<String>,
        formula: impl Into<String>,
        result_type: FormulaResultType,
    ) -> Self {
        Self {
            name: name.into(),
            formula: formula.into(),
            result_type,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormulaLimits {
    pub max_source_bytes: usize,
    pub max_ast_nodes: usize,
    pub max_nesting_depth: usize,
    pub max_evaluation_steps: usize,
    pub max_collection_elements: usize,
    pub max_regex_source_bytes: usize,
    pub max_regex_compiled_bytes: usize,
    pub max_serialized_output_bytes: usize,
}

impl Default for FormulaLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 8 * 1024,
            max_ast_nodes: 1_024,
            max_nesting_depth: 64,
            max_evaluation_steps: 50_000,
            max_collection_elements: 10_000,
            max_regex_source_bytes: 4 * 1024,
            max_regex_compiled_bytes: 1024 * 1024,
            max_serialized_output_bytes: 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FormulaValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(Decimal),
    String(String),
    List(Vec<FormulaValue>),
    Object(BTreeMap<String, FormulaValue>),
}

impl FormulaValue {
    pub fn from_json(value: &JsonValue) -> Result<Self, FormulaDiagnostic> {
        value_from_json(value, &FormulaLimits::default())
            .map_err(|error| FormulaDiagnostic::new("", error.code, error.message, error.span))
    }

    pub fn to_json(&self) -> Result<JsonValue, FormulaDiagnostic> {
        public_value_to_json(self)
            .map_err(|error| FormulaDiagnostic::new("", error.code, error.message, error.span))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormulaEvaluation {
    pub values: BTreeMap<String, FormulaValue>,
    pub errors: BTreeMap<String, FormulaDiagnostic>,
}

impl FormulaEvaluation {
    pub fn json_values(&self) -> Result<JsonMap<String, JsonValue>, FormulaDiagnostic> {
        self.values
            .iter()
            .map(|(name, value)| value.to_json().map(|value| (name.clone(), value)))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct FormulaEngine {
    limits: FormulaLimits,
}

impl Default for FormulaEngine {
    fn default() -> Self {
        Self::new(FormulaLimits::default())
    }
}

impl FormulaEngine {
    pub fn new(limits: FormulaLimits) -> Self {
        Self { limits }
    }

    pub fn limits(&self) -> &FormulaLimits {
        &self.limits
    }

    pub fn validate(
        &self,
        formula: &str,
        result_type: FormulaResultType,
    ) -> Result<(), FormulaDiagnostic> {
        let parsed = parse_formula("__validation__", formula, &self.limits)?;
        // When an expression has no field dependencies, validation can also
        // prove its declared output type and runtime allowlist behavior.
        if parsed.dependencies.is_empty() {
            let evaluation = self
                .compile([FormulaDefinition::new(
                    "__validation__",
                    formula,
                    result_type,
                )])
                .evaluate(&JsonMap::new());
            if let Some(error) = evaluation.errors.get("__validation__") {
                return Err(error.clone());
            }
        }
        Ok(())
    }

    /// Validate a Rollup expression whose only external input is the reserved
    /// `values` array. Rollup never supplies arbitrary row fields, so accepting
    /// another unresolved identifier here would defer an authoring error until
    /// materialization.
    pub fn validate_rollup(
        &self,
        formula: &str,
        result_type: FormulaResultType,
    ) -> Result<(), FormulaDiagnostic> {
        let parsed = parse_formula("__validation__", formula, &self.limits)?;
        if let Some(dependency) = parsed
            .dependencies
            .iter()
            .find(|dependency| dependency.as_str() != "values")
        {
            return Err(FormulaDiagnostic::new(
                "__validation__",
                "unknown_identifier",
                format!(
                    "unknown Rollup field or variable `{dependency}`; only `values` is available"
                ),
                None,
            ));
        }
        if parsed.dependencies.is_empty() {
            return self.validate(formula, result_type);
        }
        Ok(())
    }

    pub fn compile(
        &self,
        definitions: impl IntoIterator<Item = FormulaDefinition>,
    ) -> FormulaProgram {
        FormulaProgram::compile(definitions.into_iter().collect(), self.limits.clone())
    }
}

#[derive(Clone, Debug)]
struct ParsedFormula {
    wrapped_source: String,
    ast: JsonValue,
    dependencies: BTreeSet<String>,
}

#[derive(Clone, Debug)]
struct CompiledFormula {
    definition: FormulaDefinition,
    parsed: Option<ParsedFormula>,
    diagnostic: Option<FormulaDiagnostic>,
}

#[derive(Clone, Debug)]
pub struct FormulaProgram {
    limits: FormulaLimits,
    formulas: Vec<CompiledFormula>,
    evaluation_order: Vec<usize>,
    cyclic: BTreeSet<usize>,
    compile_diagnostics: Vec<FormulaDiagnostic>,
    fingerprint: String,
}

impl FormulaProgram {
    fn compile(definitions: Vec<FormulaDefinition>, limits: FormulaLimits) -> Self {
        let fingerprint = definition_fingerprint(&definitions);
        let mut compile_diagnostics = Vec::new();
        let mut names = BTreeMap::<String, usize>::new();
        let mut formulas = Vec::with_capacity(definitions.len());

        for definition in definitions {
            let diagnostic = if RESERVED_FORMULA_NAMES.contains(&definition.name.as_str()) {
                Some(FormulaDiagnostic::new(
                    &definition.name,
                    "reserved_field",
                    format!(
                        "`{}` is reserved and cannot be a formula field",
                        definition.name
                    ),
                    None,
                ))
            } else if definition.name.trim().is_empty() {
                Some(FormulaDiagnostic::new(
                    &definition.name,
                    "invalid_field",
                    "formula field name cannot be empty",
                    None,
                ))
            } else if names.contains_key(&definition.name) {
                Some(FormulaDiagnostic::new(
                    &definition.name,
                    "duplicate_field",
                    format!(
                        "formula field `{}` is defined more than once",
                        definition.name
                    ),
                    None,
                ))
            } else {
                None
            };

            let index = formulas.len();
            if diagnostic.is_none() {
                names.insert(definition.name.clone(), index);
            }

            let parsed = if diagnostic.is_none() {
                match parse_formula(&definition.name, &definition.formula, &limits) {
                    Ok(parsed) => Some(parsed),
                    Err(error) => {
                        compile_diagnostics.push(error.clone());
                        formulas.push(CompiledFormula {
                            definition,
                            parsed: None,
                            diagnostic: Some(error),
                        });
                        continue;
                    }
                }
            } else {
                None
            };

            if let Some(error) = &diagnostic {
                compile_diagnostics.push(error.clone());
            }
            formulas.push(CompiledFormula {
                definition,
                parsed,
                diagnostic,
            });
        }

        let mut dependencies = vec![Vec::<usize>::new(); formulas.len()];
        for (index, formula) in formulas.iter().enumerate() {
            let Some(parsed) = &formula.parsed else {
                continue;
            };
            for dependency in &parsed.dependencies {
                if let Some(&dependency_index) = names.get(dependency) {
                    dependencies[index].push(dependency_index);
                }
            }
        }

        let cyclic = (0..formulas.len())
            .filter(|&index| dependency_reaches(index, index, &dependencies, &mut BTreeSet::new()))
            .collect::<BTreeSet<_>>();

        // Topologically order every non-cyclic node. Edges from a cycle are
        // deliberately ignored here so dependents still run far enough to
        // report `dependency_failed` instead of being mislabeled as cyclic.
        let mut indegree = vec![0_usize; formulas.len()];
        let mut dependents = vec![Vec::<usize>::new(); formulas.len()];
        for (index, formula_dependencies) in dependencies.iter().enumerate() {
            if cyclic.contains(&index) {
                continue;
            }
            for &dependency in formula_dependencies {
                if !cyclic.contains(&dependency) {
                    indegree[index] += 1;
                    dependents[dependency].push(index);
                }
            }
        }
        let mut queue = VecDeque::new();
        for (index, degree) in indegree.iter().enumerate() {
            if *degree == 0 && !cyclic.contains(&index) {
                queue.push_back(index);
            }
        }
        let mut evaluation_order = Vec::with_capacity(formulas.len());
        while let Some(index) = queue.pop_front() {
            evaluation_order.push(index);
            for &dependent in &dependents[index] {
                indegree[dependent] -= 1;
                if indegree[dependent] == 0 {
                    queue.push_back(dependent);
                }
            }
        }
        for &index in &cyclic {
            let formula = &formulas[index];
            let error = FormulaDiagnostic::new(
                &formula.definition.name,
                "dependency_cycle",
                format!(
                    "formula `{}` participates in a dependency cycle",
                    formula.definition.name
                ),
                None,
            );
            compile_diagnostics.push(error);
        }

        Self {
            limits,
            formulas,
            evaluation_order,
            cyclic,
            compile_diagnostics,
            fingerprint,
        }
    }

    pub fn definitions(&self) -> impl Iterator<Item = &FormulaDefinition> {
        self.formulas.iter().map(|formula| &formula.definition)
    }

    pub(crate) fn dependencies_for(&self, field: &str) -> Option<&BTreeSet<String>> {
        self.formulas
            .iter()
            .find(|formula| formula.definition.name == field)
            .and_then(|formula| formula.parsed.as_ref())
            .map(|parsed| &parsed.dependencies)
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn compile_diagnostics(&self) -> &[FormulaDiagnostic] {
        &self.compile_diagnostics
    }

    pub fn evaluate(&self, fields: &JsonMap<String, JsonValue>) -> FormulaEvaluation {
        let mut output = FormulaEvaluation::default();
        let mut raw_fields = BTreeMap::new();
        let mut raw_input_errors = BTreeMap::new();
        let formula_names: BTreeSet<&str> = self
            .formulas
            .iter()
            .map(|formula| formula.definition.name.as_str())
            .collect();
        let mut input_budget = ValueConversionBudget {
            collection_elements: fields.len(),
        };
        for (name, value) in fields {
            // Materialized formula values are present in source frontmatter on
            // subsequent runs. Definitions own these names, so only the freshly
            // topologically evaluated values may enter the environment.
            if formula_names.contains(name.as_str()) {
                continue;
            }
            match runtime_value_from_json_inner(value, &self.limits, &mut input_budget, 0) {
                Ok(value) => {
                    raw_fields.insert(name.clone(), value);
                }
                Err(error) => {
                    raw_input_errors.insert(name.clone(), error);
                }
            }
        }

        for formula in &self.formulas {
            if let Some(error) = &formula.diagnostic {
                output
                    .errors
                    .insert(formula.definition.name.clone(), error.clone());
            }
        }

        for &index in &self.cyclic {
            let formula = &self.formulas[index];
            output.errors.insert(
                formula.definition.name.clone(),
                FormulaDiagnostic::new(
                    &formula.definition.name,
                    "dependency_cycle",
                    format!(
                        "formula `{}` participates in a dependency cycle",
                        formula.definition.name
                    ),
                    None,
                ),
            );
        }

        let mut computed = BTreeMap::<String, RuntimeValue>::new();
        for &index in &self.evaluation_order {
            let formula = &self.formulas[index];
            let name = &formula.definition.name;
            if output.errors.contains_key(name) || self.cyclic.contains(&index) {
                continue;
            }
            let Some(parsed) = &formula.parsed else {
                continue;
            };

            if let Some((field, error)) = parsed
                .dependencies
                .iter()
                .find_map(|field| raw_input_errors.get(field).map(|error| (field, error)))
            {
                output.errors.insert(
                    name.clone(),
                    FormulaDiagnostic::new(
                        name,
                        error.code.clone(),
                        format!("input field `{field}`: {}", error.message),
                        error.span.clone(),
                    ),
                );
                continue;
            }

            if let Some(dependency) = parsed.dependencies.iter().find(|dependency| {
                let dependency = dependency.as_str();
                self.formulas
                    .iter()
                    .any(|candidate| candidate.definition.name == dependency)
                    && !computed.contains_key(dependency)
            }) {
                output.errors.insert(
                    name.clone(),
                    FormulaDiagnostic::new(
                        name,
                        "dependency_failed",
                        format!("dependency `{dependency}` did not produce a value"),
                        None,
                    ),
                );
                continue;
            }

            let mut environment = raw_fields.clone();
            environment.extend(computed.clone());
            let fields_object = RuntimeValue::Object(environment.clone());
            environment.insert("fields".to_owned(), fields_object);

            let mut evaluator = Evaluator::new(&parsed.wrapped_source, &self.limits, environment);
            let value = match evaluator.eval(&parsed.ast, 0) {
                Ok(value) => value,
                Err(error) => {
                    output.errors.insert(
                        name.clone(),
                        FormulaDiagnostic::new(name, error.code, error.message, error.span),
                    );
                    continue;
                }
            };

            if let Err(error) = enforce_runtime_value_limits(&value, &self.limits) {
                output.errors.insert(
                    name.clone(),
                    FormulaDiagnostic::new(name, error.code, error.message, error.span),
                );
                continue;
            }

            let public = match validate_result(&value, formula.definition.result_type) {
                Ok(value) => value,
                Err(error) => {
                    output.errors.insert(
                        name.clone(),
                        FormulaDiagnostic::new(name, error.code, error.message, error.span),
                    );
                    continue;
                }
            };

            let serialized = match public_value_to_json(&public).and_then(|value| {
                serde_json::to_vec(&value)
                    .map_err(|error| EvalError::new("serialization_error", error.to_string(), None))
            }) {
                Ok(serialized) => serialized,
                Err(error) => {
                    output.errors.insert(
                        name.clone(),
                        FormulaDiagnostic::new(name, error.code, error.message, error.span),
                    );
                    continue;
                }
            };
            if serialized.len() > self.limits.max_serialized_output_bytes {
                output.errors.insert(
                    name.clone(),
                    FormulaDiagnostic::new(
                        name,
                        "output_limit",
                        format!(
                            "serialized formula output exceeds {} bytes",
                            self.limits.max_serialized_output_bytes
                        ),
                        None,
                    ),
                );
                continue;
            }

            computed.insert(name.clone(), value);
            output.values.insert(name.clone(), public);
        }
        output
    }
}

fn definition_fingerprint(definitions: &[FormulaDefinition]) -> String {
    let mut hasher = Sha256::new();
    for definition in definitions {
        hasher.update(definition.name.as_bytes());
        hasher.update([0]);
        hasher.update(definition.formula.as_bytes());
        hasher.update([0]);
        hasher.update(definition.result_type.to_string().as_bytes());
        hasher.update([0xff]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn dependency_reaches(
    target: usize,
    current: usize,
    dependencies: &[Vec<usize>],
    visited: &mut BTreeSet<usize>,
) -> bool {
    if !visited.insert(current) {
        return false;
    }
    for &dependency in &dependencies[current] {
        if dependency == target || dependency_reaches(target, dependency, dependencies, visited) {
            return true;
        }
    }
    false
}

fn parse_formula(
    field: &str,
    source: &str,
    limits: &FormulaLimits,
) -> Result<ParsedFormula, FormulaDiagnostic> {
    if source.len() > limits.max_source_bytes {
        return Err(FormulaDiagnostic::new(
            field,
            "source_limit",
            format!("formula source exceeds {} bytes", limits.max_source_bytes),
            None,
        ));
    }
    let wrapped_source = format!("({source})");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &wrapped_source, SourceType::default()).parse();
    if let Some(error) = parsed.diagnostics.first() {
        return Err(FormulaDiagnostic::new(
            field,
            "syntax_error",
            error.to_string(),
            None,
        ));
    }
    if parsed.program.body.len() != 1
        || !matches!(
            parsed.program.body.first(),
            Some(Statement::ExpressionStatement(_))
        )
    {
        return Err(FormulaDiagnostic::new(
            field,
            "unsupported_syntax",
            "formula must contain exactly one expression",
            None,
        ));
    }

    let ast_json = parsed.program.to_estree_json(false, false);
    let program: JsonValue = serde_json::from_str(&ast_json).map_err(|error| {
        FormulaDiagnostic::new(
            field,
            "parser_error",
            format!("could not decode parsed formula: {error}"),
            None,
        )
    })?;
    let ast = program
        .get("body")
        .and_then(JsonValue::as_array)
        .and_then(|body| body.first())
        .and_then(|statement| statement.get("expression"))
        .cloned()
        .ok_or_else(|| {
            FormulaDiagnostic::new(
                field,
                "parser_error",
                "parser did not return an expression",
                None,
            )
        })?;

    enforce_estree_limits(field, &wrapped_source, &ast, limits)?;
    let dependencies = {
        let mut validator = AstValidator::new(field, &wrapped_source, limits);
        validator.visit(&ast, 0, false)?;
        for (name, count) in &validator.identifier_references {
            let static_uses = validator
                .builtin_call_references
                .get(name)
                .copied()
                .unwrap_or(0);
            if *count > static_uses {
                validator.dependencies.insert(name.clone());
            }
        }
        validator.dependencies
    };
    Ok(ParsedFormula {
        wrapped_source,
        ast,
        dependencies,
    })
}

fn enforce_estree_limits(
    field: &str,
    source: &str,
    ast: &JsonValue,
    limits: &FormulaLimits,
) -> Result<(), FormulaDiagnostic> {
    fn visit(
        field: &str,
        source: &str,
        value: &JsonValue,
        parent_depth: usize,
        nodes: &mut usize,
        limits: &FormulaLimits,
    ) -> Result<(), FormulaDiagnostic> {
        match value {
            JsonValue::Object(object) => {
                let is_node = object.get("type").and_then(JsonValue::as_str).is_some();
                let depth = parent_depth + usize::from(is_node);
                if is_node {
                    *nodes += 1;
                    if *nodes > limits.max_ast_nodes {
                        return Err(FormulaDiagnostic::new(
                            field,
                            "ast_limit",
                            format!("formula exceeds {} AST nodes", limits.max_ast_nodes),
                            source_span(value, source),
                        ));
                    }
                    if depth > limits.max_nesting_depth {
                        return Err(FormulaDiagnostic::new(
                            field,
                            "nesting_limit",
                            format!("formula exceeds nesting depth {}", limits.max_nesting_depth),
                            source_span(value, source),
                        ));
                    }
                }
                for child in object.values() {
                    visit(field, source, child, depth, nodes, limits)?;
                }
            }
            JsonValue::Array(values) => {
                for child in values {
                    visit(field, source, child, parent_depth, nodes, limits)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    let mut nodes = 0;
    visit(field, source, ast, 0, &mut nodes, limits)
}

struct AstValidator<'a> {
    field: &'a str,
    source: &'a str,
    limits: &'a FormulaLimits,
    nodes: usize,
    dependencies: BTreeSet<String>,
    identifier_references: BTreeMap<String, usize>,
    builtin_call_references: BTreeMap<String, usize>,
    locals: Vec<BTreeSet<String>>,
}

impl<'a> AstValidator<'a> {
    fn new(field: &'a str, source: &'a str, limits: &'a FormulaLimits) -> Self {
        Self {
            field,
            source,
            limits,
            nodes: 0,
            dependencies: BTreeSet::new(),
            identifier_references: BTreeMap::new(),
            builtin_call_references: BTreeMap::new(),
            locals: Vec::new(),
        }
    }

    fn visit(
        &mut self,
        node: &JsonValue,
        depth: usize,
        arrow_allowed: bool,
    ) -> Result<(), FormulaDiagnostic> {
        self.nodes += 1;
        if self.nodes > self.limits.max_ast_nodes {
            return Err(self.error(
                "ast_limit",
                format!("formula exceeds {} AST nodes", self.limits.max_ast_nodes),
                node,
            ));
        }
        if depth > self.limits.max_nesting_depth {
            return Err(self.error(
                "nesting_limit",
                format!(
                    "formula exceeds nesting depth {}",
                    self.limits.max_nesting_depth
                ),
                node,
            ));
        }

        let kind = node_type(node)
            .ok_or_else(|| self.error("parser_error", "AST node has no type", node))?;
        match kind {
            "Literal" | "NumericLiteral" | "StringLiteral" | "BooleanLiteral" | "NullLiteral"
            | "RegExpLiteral" => {
                if node.get("regex").is_some() || kind == "RegExpLiteral" {
                    let regex = node.get("regex");
                    let pattern = regex
                        .and_then(|regex| regex.get("pattern"))
                        .or_else(|| node.get("pattern"))
                        .and_then(JsonValue::as_str)
                        .ok_or_else(|| self.error("parser_error", "regex has no pattern", node))?;
                    let flags = regex
                        .and_then(|regex| regex.get("flags"))
                        .or_else(|| node.get("flags"))
                        .and_then(JsonValue::as_str)
                        .unwrap_or("");
                    compile_regex(pattern, flags, self.limits, source_span(node, self.source))
                        .map_err(|error| {
                            FormulaDiagnostic::new(
                                self.field,
                                error.code,
                                error.message,
                                error.span,
                            )
                        })?;
                }
            }
            "Identifier" => {
                let name = node_string(node, "name")
                    .ok_or_else(|| self.error("parser_error", "identifier has no name", node))?;
                if FORBIDDEN_IDENTIFIERS.contains(&name) {
                    return Err(self.error(
                        "forbidden_identifier",
                        format!("`{name}` is not available in formulas"),
                        node,
                    ));
                }
                if !self.is_local(name) && !BUILTIN_ROOTS.contains(&name) && name != "fields" {
                    self.dependencies.insert(name.to_owned());
                }
                if !self.is_local(name) && name != "fields" {
                    *self
                        .identifier_references
                        .entry(name.to_owned())
                        .or_default() += 1;
                }
            }
            "TemplateLiteral" => {
                self.visit_array(node, "expressions", depth)?;
            }
            "ArrayExpression" => {
                let elements = node
                    .get("elements")
                    .and_then(JsonValue::as_array)
                    .ok_or_else(|| self.error("parser_error", "array has no elements", node))?;
                if elements.len() > self.limits.max_collection_elements {
                    return Err(self.error(
                        "collection_limit",
                        format!(
                            "array literal exceeds {} elements",
                            self.limits.max_collection_elements
                        ),
                        node,
                    ));
                }
                for element in elements.iter().filter(|element| !element.is_null()) {
                    if node_type(element) == Some("SpreadElement") {
                        return Err(self.error(
                            "unsupported_syntax",
                            "spread elements are not supported",
                            element,
                        ));
                    }
                    self.visit(element, depth + 1, false)?;
                }
            }
            "ObjectExpression" => {
                let properties = node
                    .get("properties")
                    .and_then(JsonValue::as_array)
                    .ok_or_else(|| self.error("parser_error", "object has no properties", node))?;
                if properties.len() > self.limits.max_collection_elements {
                    return Err(self.error(
                        "collection_limit",
                        format!(
                            "object literal exceeds {} properties",
                            self.limits.max_collection_elements
                        ),
                        node,
                    ));
                }
                for property in properties {
                    if node_type(property) != Some("Property") {
                        return Err(self.error(
                            "unsupported_syntax",
                            "object spread and methods are not supported",
                            property,
                        ));
                    }
                    if property.get("kind").and_then(JsonValue::as_str) != Some("init")
                        || property
                            .get("method")
                            .and_then(JsonValue::as_bool)
                            .unwrap_or(false)
                    {
                        return Err(self.error(
                            "unsupported_syntax",
                            "getters, setters, and object methods are not supported",
                            property,
                        ));
                    }
                    let computed = property
                        .get("computed")
                        .and_then(JsonValue::as_bool)
                        .unwrap_or(false);
                    if computed {
                        if let Some(key) = property.get("key") {
                            self.visit(key, depth + 1, false)?;
                        }
                    } else if let Some(key) = static_property_name(property.get("key")) {
                        reject_property_name(self.field, self.source, property, &key)?;
                    }
                    let value = property.get("value").ok_or_else(|| {
                        self.error("parser_error", "object property has no value", property)
                    })?;
                    self.visit(value, depth + 1, false)?;
                }
            }
            "UnaryExpression" => {
                let operator = node_string(node, "operator").unwrap_or("");
                if !matches!(operator, "!" | "+" | "-" | "typeof") {
                    return Err(self.error(
                        "unsupported_operator",
                        format!("unary operator `{operator}` is not supported"),
                        node,
                    ));
                }
                self.visit_required(node, "argument", depth)?;
            }
            "BinaryExpression" => {
                let operator = node_string(node, "operator").unwrap_or("");
                if !matches!(
                    operator,
                    "+" | "-" | "*" | "/" | "%" | "**" | "===" | "!==" | "<" | "<=" | ">" | ">="
                ) {
                    let code = if matches!(operator, "==" | "!=") {
                        "loose_equality"
                    } else {
                        "unsupported_operator"
                    };
                    return Err(self.error(
                        code,
                        format!("binary operator `{operator}` is not supported"),
                        node,
                    ));
                }
                self.visit_required(node, "left", depth)?;
                self.visit_required(node, "right", depth)?;
            }
            "LogicalExpression" => {
                let operator = node_string(node, "operator").unwrap_or("");
                if !matches!(operator, "&&" | "||" | "??") {
                    return Err(self.error(
                        "unsupported_operator",
                        format!("logical operator `{operator}` is not supported"),
                        node,
                    ));
                }
                self.visit_required(node, "left", depth)?;
                self.visit_required(node, "right", depth)?;
            }
            "ConditionalExpression" => {
                self.visit_required(node, "test", depth)?;
                self.visit_required(node, "consequent", depth)?;
                self.visit_required(node, "alternate", depth)?;
            }
            "MemberExpression" | "StaticMemberExpression" | "ComputedMemberExpression" => {
                self.visit_member(node, depth)?;
            }
            "ChainExpression" => {
                self.visit_required(node, "expression", depth)?;
            }
            "CallExpression" => {
                let callee = node
                    .get("callee")
                    .ok_or_else(|| self.error("parser_error", "call has no callee", node))?;
                match node_type(callee) {
                    Some("Identifier") => {
                        let name = identifier_name(callee).unwrap_or_default();
                        if FORBIDDEN_IDENTIFIERS.contains(&name) {
                            return Err(self.error(
                                "forbidden_identifier",
                                format!("`{name}` is not available in formulas"),
                                callee,
                            ));
                        }
                        if !is_allowed_global_call(name) {
                            return Err(self.error(
                                "unsupported_call",
                                format!("function `{name}` is not available in formulas"),
                                callee,
                            ));
                        }
                        *self
                            .builtin_call_references
                            .entry(name.to_owned())
                            .or_default() += 1;
                    }
                    Some(
                        "MemberExpression" | "StaticMemberExpression" | "ComputedMemberExpression",
                    ) => {
                        let Some(method) = statically_called_method_name(callee) else {
                            return Err(self.error(
                                "unsupported_call",
                                "computed method calls require a literal method name",
                                callee,
                            ));
                        };
                        reject_property_name(self.field, self.source, callee, &method)?;
                        if let Some(root) =
                            callee
                                .get("object")
                                .and_then(identifier_name)
                                .filter(|name| {
                                    matches!(
                                        *name,
                                        "Math"
                                            | "Number"
                                            | "String"
                                            | "Array"
                                            | "Object"
                                            | "JSON"
                                            | "Date"
                                    )
                                })
                        {
                            if !is_allowed_static_call(root, &method) {
                                return Err(self.error(
                                    "unsupported_call",
                                    format!("`{root}.{method}` is not available in formulas"),
                                    callee,
                                ));
                            }
                            *self
                                .builtin_call_references
                                .entry(root.to_owned())
                                .or_default() += 1;
                        } else if !is_allowed_instance_call(&method) {
                            return Err(self.error(
                                "unsupported_call",
                                format!("method `{method}` is not available in formulas"),
                                callee,
                            ));
                        }
                    }
                    _ => {
                        return Err(self.error(
                            "unsupported_call",
                            "only allowlisted functions and methods can be called",
                            callee,
                        ));
                    }
                }
                self.visit(callee, depth + 1, false)?;
                let callback_allowed = statically_called_method_name(callee)
                    .is_some_and(|name| CALLBACK_METHODS.contains(&name.as_str()));
                let arguments = node
                    .get("arguments")
                    .and_then(JsonValue::as_array)
                    .ok_or_else(|| self.error("parser_error", "call has no arguments", node))?;
                for argument in arguments {
                    if node_type(argument) == Some("SpreadElement") {
                        return Err(self.error(
                            "unsupported_syntax",
                            "spread arguments are not supported",
                            argument,
                        ));
                    }
                    self.visit(argument, depth + 1, callback_allowed)?;
                }
            }
            "ArrowFunctionExpression" => {
                if !arrow_allowed {
                    return Err(self.error(
                        "unsupported_callback",
                        "arrow callbacks are only allowed in map, filter, reduce, some, every, and find",
                        node,
                    ));
                }
                if node
                    .get("async")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false)
                    || node
                        .get("generator")
                        .and_then(JsonValue::as_bool)
                        .unwrap_or(false)
                {
                    return Err(self.error(
                        "unsupported_callback",
                        "async and generator callbacks are not supported",
                        node,
                    ));
                }
                let body = node.get("body").ok_or_else(|| {
                    self.error("parser_error", "arrow callback has no body", node)
                })?;
                if node_type(body) == Some("BlockStatement") {
                    return Err(self.error(
                        "unsupported_callback",
                        "callback bodies must be a single expression",
                        body,
                    ));
                }
                let params = node
                    .get("params")
                    .and_then(JsonValue::as_array)
                    .ok_or_else(|| {
                        self.error("parser_error", "callback has no parameters", node)
                    })?;
                if params.len() > 4 {
                    return Err(self.error(
                        "unsupported_callback",
                        "callbacks accept at most four parameters",
                        node,
                    ));
                }
                let mut locals = BTreeSet::new();
                for parameter in params {
                    if node_type(parameter) != Some("Identifier") {
                        return Err(self.error(
                            "unsupported_callback",
                            "callback parameters must be plain identifiers",
                            parameter,
                        ));
                    }
                    if let Some(name) = node_string(parameter, "name") {
                        locals.insert(name.to_owned());
                    }
                }
                self.locals.push(locals);
                let result = self.visit(body, depth + 1, false);
                self.locals.pop();
                result?;
            }
            "ParenthesizedExpression" => {
                self.visit_required(node, "expression", depth)?;
            }
            unsupported => {
                return Err(self.error(
                    "unsupported_syntax",
                    format!("`{unsupported}` is not supported in formulas"),
                    node,
                ));
            }
        }
        Ok(())
    }

    fn visit_member(&mut self, node: &JsonValue, depth: usize) -> Result<(), FormulaDiagnostic> {
        self.visit_required(node, "object", depth)?;
        let computed = node
            .get("computed")
            .and_then(JsonValue::as_bool)
            .unwrap_or(node_type(node) == Some("ComputedMemberExpression"));
        if computed {
            self.visit_required(node, "property", depth)?;
            if let Some(name) = static_property_name(node.get("property")) {
                reject_property_name(self.field, self.source, node, &name)?;
                self.record_fields_dependency(node, &name);
            }
        } else if let Some(name) = static_property_name(node.get("property")) {
            reject_property_name(self.field, self.source, node, &name)?;
            self.record_fields_dependency(node, &name);
        }
        Ok(())
    }

    fn record_fields_dependency(&mut self, member: &JsonValue, property: &str) {
        if member
            .get("object")
            .is_some_and(|object| node_type(object) == Some("Identifier"))
            && member
                .get("object")
                .and_then(|object| node_string(object, "name"))
                == Some("fields")
        {
            self.dependencies.insert(property.to_owned());
        }
    }

    fn visit_required(
        &mut self,
        node: &JsonValue,
        key: &str,
        depth: usize,
    ) -> Result<(), FormulaDiagnostic> {
        let child = node
            .get(key)
            .ok_or_else(|| self.error("parser_error", format!("AST node has no `{key}`"), node))?;
        self.visit(child, depth + 1, false)
    }

    fn visit_array(
        &mut self,
        node: &JsonValue,
        key: &str,
        depth: usize,
    ) -> Result<(), FormulaDiagnostic> {
        let children = node.get(key).and_then(JsonValue::as_array).ok_or_else(|| {
            self.error(
                "parser_error",
                format!("AST node has no `{key}` array"),
                node,
            )
        })?;
        for child in children {
            self.visit(child, depth + 1, false)?;
        }
        Ok(())
    }

    fn is_local(&self, name: &str) -> bool {
        self.locals.iter().rev().any(|scope| scope.contains(name))
    }

    fn error(
        &self,
        code: impl Into<String>,
        message: impl Into<String>,
        node: &JsonValue,
    ) -> FormulaDiagnostic {
        FormulaDiagnostic::new(self.field, code, message, source_span(node, self.source))
    }
}

fn reject_property_name(
    field: &str,
    source: &str,
    node: &JsonValue,
    name: &str,
) -> Result<(), FormulaDiagnostic> {
    if FORBIDDEN_PROPERTIES.contains(&name) {
        return Err(FormulaDiagnostic::new(
            field,
            "forbidden_property",
            format!("property `{name}` is not available in formulas"),
            source_span(node, source),
        ));
    }
    Ok(())
}

fn node_type(node: &JsonValue) -> Option<&str> {
    node.get("type").and_then(JsonValue::as_str)
}

fn node_string<'a>(node: &'a JsonValue, key: &str) -> Option<&'a str> {
    node.get(key).and_then(JsonValue::as_str)
}

fn source_span(node: &JsonValue, wrapped_source: &str) -> Option<FormulaSourceSpan> {
    let (start, end) = if let (Some(start), Some(end)) = (
        node.get("start").and_then(JsonValue::as_u64),
        node.get("end").and_then(JsonValue::as_u64),
    ) {
        (start, end)
    } else {
        let range = node.get("range")?.as_array()?;
        (range.first()?.as_u64()?, range.get(1)?.as_u64()?)
    };
    let max = wrapped_source.len().saturating_sub(2) as u64;
    Some(FormulaSourceSpan {
        start: start.saturating_sub(1).min(max) as u32,
        end: end.saturating_sub(1).min(max) as u32,
    })
}

fn static_property_name(node: Option<&JsonValue>) -> Option<String> {
    let node = node?;
    match node_type(node) {
        Some("Identifier") => node_string(node, "name").map(str::to_owned),
        Some("Literal") | Some("StringLiteral") => node
            .get("value")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

fn statically_called_method_name(node: &JsonValue) -> Option<String> {
    if !matches!(
        node_type(node),
        Some("MemberExpression" | "StaticMemberExpression" | "ComputedMemberExpression")
    ) {
        return None;
    }
    let computed = node
        .get("computed")
        .and_then(JsonValue::as_bool)
        .unwrap_or(node_type(node) == Some("ComputedMemberExpression"));
    let property = node.get("property")?;
    if computed && node_type(property) == Some("Identifier") {
        return None;
    }
    static_property_name(Some(property))
}

#[derive(Clone, Debug)]
enum RuntimeValue {
    Undefined,
    /// Internal sentinel that propagates through the remainder of one optional
    /// chain and becomes ordinary `undefined` at `ChainExpression`.
    OptionalShortCircuit,
    Null,
    Boolean(bool),
    Number(Decimal),
    String(String),
    List(Vec<RuntimeValue>),
    Object(BTreeMap<String, RuntimeValue>),
    Regex(RegexValue),
    Callback(CallbackValue),
}

#[derive(Clone, Debug)]
struct RegexValue {
    pattern: String,
    flags: String,
    regex: Regex,
}

#[derive(Clone, Debug)]
struct CallbackValue {
    params: Vec<String>,
    body: JsonValue,
}

impl RuntimeValue {
    fn type_name(&self) -> &'static str {
        match self {
            Self::Undefined | Self::OptionalShortCircuit => "undefined",
            Self::Null => "null",
            Self::Boolean(_) => "boolean",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::List(_) => "array",
            Self::Object(_) => "object",
            Self::Regex(_) => "regexp",
            Self::Callback(_) => "function",
        }
    }

    fn is_nullish(&self) -> bool {
        matches!(
            self,
            Self::Null | Self::Undefined | Self::OptionalShortCircuit
        )
    }

    fn truthy(&self) -> bool {
        match self {
            Self::Undefined | Self::OptionalShortCircuit | Self::Null => false,
            Self::Boolean(value) => *value,
            Self::Number(value) => !value.is_zero(),
            Self::String(value) => !value.is_empty(),
            Self::List(_) | Self::Object(_) | Self::Regex(_) | Self::Callback(_) => true,
        }
    }

    fn to_public(&self) -> Result<FormulaValue, EvalError> {
        match self {
            Self::Undefined | Self::OptionalShortCircuit => Ok(FormulaValue::Undefined),
            Self::Null => Ok(FormulaValue::Null),
            Self::Boolean(value) => Ok(FormulaValue::Boolean(*value)),
            Self::Number(value) => Ok(FormulaValue::Number(*value)),
            Self::String(value) => Ok(FormulaValue::String(value.clone())),
            Self::List(values) => values
                .iter()
                .map(Self::to_public)
                .collect::<Result<Vec<_>, _>>()
                .map(FormulaValue::List),
            Self::Object(values) => values
                .iter()
                .map(|(name, value)| value.to_public().map(|value| (name.clone(), value)))
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map(FormulaValue::Object),
            Self::Regex(_) | Self::Callback(_) => Err(EvalError::new(
                "result_type",
                format!("{} cannot be returned as a formula value", self.type_name()),
                None,
            )),
        }
    }
}

#[derive(Clone, Debug)]
struct EvalError {
    code: String,
    message: String,
    span: Option<FormulaSourceSpan>,
}

impl EvalError {
    fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        span: Option<FormulaSourceSpan>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            span,
        }
    }
}

#[derive(Default)]
struct ValueConversionBudget {
    collection_elements: usize,
}

fn runtime_value_from_json(
    value: &JsonValue,
    limits: &FormulaLimits,
) -> Result<RuntimeValue, EvalError> {
    runtime_value_from_json_inner(value, limits, &mut ValueConversionBudget::default(), 0)
}

fn runtime_value_from_json_inner(
    value: &JsonValue,
    limits: &FormulaLimits,
    budget: &mut ValueConversionBudget,
    depth: usize,
) -> Result<RuntimeValue, EvalError> {
    if budget.collection_elements > limits.max_collection_elements {
        return Err(EvalError::new(
            "collection_limit",
            format!(
                "input exceeds {} total collection elements",
                limits.max_collection_elements
            ),
            None,
        ));
    }
    if depth > limits.max_nesting_depth {
        return Err(EvalError::new(
            "nesting_limit",
            format!("input exceeds nesting depth {}", limits.max_nesting_depth),
            None,
        ));
    }
    match value {
        JsonValue::Null => Ok(RuntimeValue::Null),
        JsonValue::Bool(value) => Ok(RuntimeValue::Boolean(*value)),
        JsonValue::Number(value) => decimal_from_str(value.as_str())
            .map(RuntimeValue::Number)
            .map_err(|_| {
                EvalError::new(
                    "number_overflow",
                    format!("number `{value}` is outside the supported decimal range"),
                    None,
                )
            }),
        JsonValue::String(value) => Ok(RuntimeValue::String(value.clone())),
        JsonValue::Array(values) => {
            budget.collection_elements = budget.collection_elements.saturating_add(values.len());
            if budget.collection_elements > limits.max_collection_elements {
                return Err(EvalError::new(
                    "collection_limit",
                    format!(
                        "input exceeds {} total collection elements",
                        limits.max_collection_elements
                    ),
                    None,
                ));
            }
            values
                .iter()
                .map(|value| runtime_value_from_json_inner(value, limits, budget, depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(RuntimeValue::List)
        }
        JsonValue::Object(values) => {
            budget.collection_elements = budget.collection_elements.saturating_add(values.len());
            if budget.collection_elements > limits.max_collection_elements {
                return Err(EvalError::new(
                    "collection_limit",
                    format!(
                        "input exceeds {} total collection elements",
                        limits.max_collection_elements
                    ),
                    None,
                ));
            }
            values
                .iter()
                .map(|(name, value)| {
                    runtime_value_from_json_inner(value, limits, budget, depth + 1)
                        .map(|value| (name.clone(), value))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map(RuntimeValue::Object)
        }
    }
}

fn value_from_json(value: &JsonValue, limits: &FormulaLimits) -> Result<FormulaValue, EvalError> {
    runtime_value_from_json(value, limits)?.to_public()
}

fn public_value_to_json(value: &FormulaValue) -> Result<JsonValue, EvalError> {
    match value {
        FormulaValue::Undefined => Err(EvalError::new(
            "undefined_result",
            "undefined cannot be serialized",
            None,
        )),
        FormulaValue::Null => Ok(JsonValue::Null),
        FormulaValue::Boolean(value) => Ok(JsonValue::Bool(*value)),
        FormulaValue::Number(value) => {
            let normalized = value.normalize().to_string();
            serde_json::Number::from_str(&normalized)
                .map(JsonValue::Number)
                .map_err(|_| {
                    EvalError::new(
                        "serialization_error",
                        format!("could not serialize decimal `{normalized}` as JSON"),
                        None,
                    )
                })
        }
        FormulaValue::String(value) => Ok(JsonValue::String(value.clone())),
        FormulaValue::List(values) => values
            .iter()
            .map(public_value_to_json)
            .collect::<Result<Vec<_>, _>>()
            .map(JsonValue::Array),
        FormulaValue::Object(values) => values
            .iter()
            .map(|(name, value)| public_value_to_json(value).map(|value| (name.clone(), value)))
            .collect::<Result<JsonMap<_, _>, _>>()
            .map(JsonValue::Object),
    }
}

fn decimal_from_str(source: &str) -> Result<Decimal, rust_decimal::Error> {
    if source.contains(['e', 'E']) {
        Decimal::from_scientific(source)
    } else {
        Decimal::from_str(source)
    }
}

struct Evaluator<'a> {
    source: &'a str,
    limits: &'a FormulaLimits,
    globals: BTreeMap<String, RuntimeValue>,
    scopes: Vec<BTreeMap<String, RuntimeValue>>,
    steps: usize,
}

impl<'a> Evaluator<'a> {
    fn new(
        source: &'a str,
        limits: &'a FormulaLimits,
        globals: BTreeMap<String, RuntimeValue>,
    ) -> Self {
        Self {
            source,
            limits,
            globals,
            scopes: Vec::new(),
            steps: 0,
        }
    }

    fn eval(&mut self, node: &JsonValue, depth: usize) -> Result<RuntimeValue, EvalError> {
        self.step(node)?;
        if depth > self.limits.max_nesting_depth {
            return Err(self.error(
                "nesting_limit",
                format!(
                    "formula exceeds nesting depth {}",
                    self.limits.max_nesting_depth
                ),
                node,
            ));
        }
        let kind = node_type(node)
            .ok_or_else(|| self.error("parser_error", "AST node has no type", node))?;
        match kind {
            "Literal" => self.eval_literal(node),
            "NumericLiteral" => self.eval_number_literal(node),
            "StringLiteral" => node
                .get("value")
                .and_then(JsonValue::as_str)
                .map(|value| RuntimeValue::String(value.to_owned()))
                .ok_or_else(|| self.error("parser_error", "string literal has no value", node)),
            "BooleanLiteral" => node
                .get("value")
                .and_then(JsonValue::as_bool)
                .map(RuntimeValue::Boolean)
                .ok_or_else(|| self.error("parser_error", "boolean literal has no value", node)),
            "NullLiteral" => Ok(RuntimeValue::Null),
            "RegExpLiteral" => self.eval_regex_literal(node),
            "Identifier" => self.eval_identifier(node),
            "TemplateLiteral" => self.eval_template(node, depth),
            "ArrayExpression" => self.eval_array(node, depth),
            "ObjectExpression" => self.eval_object(node, depth),
            "UnaryExpression" => self.eval_unary(node, depth),
            "BinaryExpression" => self.eval_binary(node, depth),
            "LogicalExpression" => self.eval_logical(node, depth),
            "ConditionalExpression" => {
                let test = self.eval_required(node, "test", depth)?;
                if test.truthy() {
                    self.eval_required(node, "consequent", depth)
                } else {
                    self.eval_required(node, "alternate", depth)
                }
            }
            "MemberExpression" | "StaticMemberExpression" | "ComputedMemberExpression" => {
                self.eval_member(node, depth)
            }
            "ChainExpression" => {
                self.eval_required(node, "expression", depth)
                    .map(|value| match value {
                        RuntimeValue::OptionalShortCircuit => RuntimeValue::Undefined,
                        value => value,
                    })
            }
            "ParenthesizedExpression" => self.eval_required(node, "expression", depth),
            "CallExpression" => self.eval_call(node, depth),
            "ArrowFunctionExpression" => self.eval_arrow(node),
            unsupported => Err(self.error(
                "unsupported_syntax",
                format!("`{unsupported}` is not supported in formulas"),
                node,
            )),
        }
    }

    fn eval_literal(&mut self, node: &JsonValue) -> Result<RuntimeValue, EvalError> {
        if node.get("regex").is_some() {
            return self.eval_regex_literal(node);
        }
        match node.get("value") {
            Some(JsonValue::Null) => Ok(RuntimeValue::Null),
            Some(JsonValue::Bool(value)) => Ok(RuntimeValue::Boolean(*value)),
            Some(JsonValue::String(value)) => Ok(RuntimeValue::String(value.clone())),
            Some(JsonValue::Number(_)) => self.eval_number_literal(node),
            _ => Err(self.error("parser_error", "literal has no supported value", node)),
        }
    }

    fn eval_number_literal(&self, node: &JsonValue) -> Result<RuntimeValue, EvalError> {
        let raw = node
            .get("raw")
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
            .or_else(|| self.source_slice(node).map(str::to_owned))
            .or_else(|| node.get("value").map(JsonValue::to_string))
            .ok_or_else(|| self.error("parser_error", "number literal has no value", node))?;
        decimal_from_str(raw.trim())
            .map(RuntimeValue::Number)
            .map_err(|_| {
                self.error(
                    "number_overflow",
                    format!("number `{raw}` is outside the supported decimal range"),
                    node,
                )
            })
    }

    fn eval_regex_literal(&self, node: &JsonValue) -> Result<RuntimeValue, EvalError> {
        let regex = node.get("regex");
        let pattern = regex
            .and_then(|regex| regex.get("pattern"))
            .or_else(|| node.get("pattern"))
            .and_then(JsonValue::as_str)
            .ok_or_else(|| self.error("parser_error", "regex has no pattern", node))?;
        let flags = regex
            .and_then(|regex| regex.get("flags"))
            .or_else(|| node.get("flags"))
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        compile_regex(pattern, flags, self.limits, source_span(node, self.source))
            .map(RuntimeValue::Regex)
    }

    fn eval_identifier(&self, node: &JsonValue) -> Result<RuntimeValue, EvalError> {
        let name = node_string(node, "name")
            .ok_or_else(|| self.error("parser_error", "identifier has no name", node))?;
        if name == "undefined" {
            return Ok(RuntimeValue::Undefined);
        }
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return Ok(value.clone());
            }
        }
        self.globals.get(name).cloned().ok_or_else(|| {
            self.error(
                "unknown_identifier",
                format!("unknown field or variable `{name}`"),
                node,
            )
        })
    }

    fn eval_template(&mut self, node: &JsonValue, depth: usize) -> Result<RuntimeValue, EvalError> {
        let quasis = node
            .get("quasis")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| self.error("parser_error", "template has no quasis", node))?;
        let expressions = node
            .get("expressions")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| self.error("parser_error", "template has no expressions", node))?;
        let mut output = String::new();
        for (index, quasi) in quasis.iter().enumerate() {
            let cooked = quasi
                .get("value")
                .and_then(|value| value.get("cooked"))
                .and_then(JsonValue::as_str)
                .or_else(|| {
                    quasi
                        .get("value")
                        .and_then(|value| value.get("raw"))
                        .and_then(JsonValue::as_str)
                })
                .unwrap_or("");
            output.push_str(cooked);
            if let Some(expression) = expressions.get(index) {
                let value = self.eval(expression, depth + 1)?;
                output.push_str(&js_string(&value)?);
            }
            self.ensure_output_string(&output, node)?;
        }
        Ok(RuntimeValue::String(output))
    }

    fn eval_array(&mut self, node: &JsonValue, depth: usize) -> Result<RuntimeValue, EvalError> {
        let elements = node
            .get("elements")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| self.error("parser_error", "array has no elements", node))?;
        self.ensure_collection(elements.len(), node)?;
        let mut output = Vec::with_capacity(elements.len());
        let mut total_elements = elements.len();
        let mut text_bytes = 0usize;
        for element in elements {
            let value = if element.is_null() {
                RuntimeValue::Undefined
            } else {
                self.eval(element, depth + 1)?
            };
            let (nested_elements, nested_text) = runtime_value_usage(&value, self.limits)?;
            total_elements = total_elements.saturating_add(nested_elements);
            text_bytes = text_bytes.saturating_add(nested_text);
            if total_elements > self.limits.max_collection_elements {
                return Err(self.error(
                    "collection_limit",
                    format!(
                        "array exceeds {} total collection elements",
                        self.limits.max_collection_elements
                    ),
                    node,
                ));
            }
            if text_bytes > self.limits.max_serialized_output_bytes {
                return Err(self.error(
                    "output_limit",
                    "array exceeds the serialized output budget",
                    node,
                ));
            }
            output.push(value);
        }
        Ok(RuntimeValue::List(output))
    }

    fn eval_object(&mut self, node: &JsonValue, depth: usize) -> Result<RuntimeValue, EvalError> {
        let properties = node
            .get("properties")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| self.error("parser_error", "object has no properties", node))?;
        self.ensure_collection(properties.len(), node)?;
        let mut output = BTreeMap::new();
        let mut total_elements = properties.len();
        let mut text_bytes = 0usize;
        for property in properties {
            let computed = property
                .get("computed")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            let key = if computed {
                let value = self.eval_required(property, "key", depth)?;
                property_key(&value)?
            } else {
                static_property_name(property.get("key")).ok_or_else(|| {
                    self.error("parser_error", "object property has no key", property)
                })?
            };
            if FORBIDDEN_PROPERTIES.contains(&key.as_str()) {
                return Err(self.error(
                    "forbidden_property",
                    format!("property `{key}` is not available in formulas"),
                    property,
                ));
            }
            let value = self.eval_required(property, "value", depth)?;
            let (nested_elements, nested_text) = runtime_value_usage(&value, self.limits)?;
            total_elements = total_elements.saturating_add(nested_elements);
            text_bytes = text_bytes
                .saturating_add(key.len())
                .saturating_add(nested_text);
            if total_elements > self.limits.max_collection_elements {
                return Err(self.error(
                    "collection_limit",
                    format!(
                        "object exceeds {} total collection elements",
                        self.limits.max_collection_elements
                    ),
                    node,
                ));
            }
            if text_bytes > self.limits.max_serialized_output_bytes {
                return Err(self.error(
                    "output_limit",
                    "object exceeds the serialized output budget",
                    node,
                ));
            }
            output.insert(key, value);
        }
        Ok(RuntimeValue::Object(output))
    }

    fn eval_unary(&mut self, node: &JsonValue, depth: usize) -> Result<RuntimeValue, EvalError> {
        let operator = node_string(node, "operator").unwrap_or("");
        let value = self.eval_required(node, "argument", depth)?;
        match operator {
            "!" => Ok(RuntimeValue::Boolean(!value.truthy())),
            "+" => number_conversion(&value).map(RuntimeValue::Number),
            "-" => expect_number(&value)
                .and_then(|value| Decimal::ZERO.checked_sub(value).ok_or_else(number_overflow))
                .map(RuntimeValue::Number),
            "typeof" => Ok(RuntimeValue::String(
                match value {
                    RuntimeValue::Undefined | RuntimeValue::OptionalShortCircuit => "undefined",
                    RuntimeValue::Null | RuntimeValue::Object(_) | RuntimeValue::List(_) => {
                        "object"
                    }
                    RuntimeValue::Boolean(_) => "boolean",
                    RuntimeValue::Number(_) => "number",
                    RuntimeValue::String(_) => "string",
                    RuntimeValue::Regex(_) => "object",
                    RuntimeValue::Callback(_) => "function",
                }
                .to_owned(),
            )),
            _ => Err(self.error(
                "unsupported_operator",
                format!("unary operator `{operator}` is not supported"),
                node,
            )),
        }
        .map_err(|mut error| {
            if error.span.is_none() {
                error.span = source_span(node, self.source);
            }
            error
        })
    }

    fn eval_binary(&mut self, node: &JsonValue, depth: usize) -> Result<RuntimeValue, EvalError> {
        let left = self.eval_required(node, "left", depth)?;
        let right = self.eval_required(node, "right", depth)?;
        let operator = node_string(node, "operator").unwrap_or("");
        let result = match operator {
            "+" => add_values(left, right),
            "-" => checked_numeric_binary(left, right, Decimal::checked_sub),
            "*" => checked_numeric_binary(left, right, Decimal::checked_mul),
            "/" => {
                if matches!(&right, RuntimeValue::Number(value) if value.is_zero()) {
                    Err(EvalError::new("division_by_zero", "division by zero", None))
                } else {
                    checked_numeric_binary(left, right, Decimal::checked_div)
                }
            }
            "%" => {
                if matches!(&right, RuntimeValue::Number(value) if value.is_zero()) {
                    Err(EvalError::new(
                        "division_by_zero",
                        "remainder by zero",
                        None,
                    ))
                } else {
                    checked_numeric_binary(left, right, Decimal::checked_rem)
                }
            }
            "**" => {
                let base = expect_number(&left)?;
                let exponent = expect_number(&right)?;
                base.checked_powd(exponent)
                    .map(RuntimeValue::Number)
                    .ok_or_else(number_overflow)
            }
            "===" => Ok(RuntimeValue::Boolean(strict_equal(&left, &right))),
            "!==" => Ok(RuntimeValue::Boolean(!strict_equal(&left, &right))),
            "<" | "<=" | ">" | ">=" => {
                let ordering = compare_values(&left, &right)?;
                Ok(RuntimeValue::Boolean(match operator {
                    "<" => ordering == Ordering::Less,
                    "<=" => ordering != Ordering::Greater,
                    ">" => ordering == Ordering::Greater,
                    ">=" => ordering != Ordering::Less,
                    _ => false,
                }))
            }
            _ => Err(EvalError::new(
                "unsupported_operator",
                format!("binary operator `{operator}` is not supported"),
                None,
            )),
        };
        result.map_err(|mut error| {
            if error.span.is_none() {
                error.span = source_span(node, self.source);
            }
            error
        })
    }

    fn eval_logical(&mut self, node: &JsonValue, depth: usize) -> Result<RuntimeValue, EvalError> {
        let left = self.eval_required(node, "left", depth)?;
        match node_string(node, "operator").unwrap_or("") {
            "&&" if !left.truthy() => Ok(left),
            "||" if left.truthy() => Ok(left),
            "??" if !left.is_nullish() => Ok(left),
            "&&" | "||" | "??" => self.eval_required(node, "right", depth),
            operator => Err(self.error(
                "unsupported_operator",
                format!("logical operator `{operator}` is not supported"),
                node,
            )),
        }
    }

    fn eval_member(&mut self, node: &JsonValue, depth: usize) -> Result<RuntimeValue, EvalError> {
        let object = self.eval_required(node, "object", depth)?;
        if matches!(object, RuntimeValue::OptionalShortCircuit) {
            return Ok(RuntimeValue::OptionalShortCircuit);
        }
        let optional = node
            .get("optional")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        if object.is_nullish() && optional {
            return Ok(RuntimeValue::OptionalShortCircuit);
        }
        let key = self.member_key(node, depth)?;
        get_property(&object, &key, optional).map_err(|mut error| {
            if error.span.is_none() {
                error.span = source_span(node, self.source);
            }
            error
        })
    }

    fn eval_arrow(&self, node: &JsonValue) -> Result<RuntimeValue, EvalError> {
        let params = node
            .get("params")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| self.error("parser_error", "callback has no parameters", node))?
            .iter()
            .map(|param| {
                node_string(param, "name")
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        self.error(
                            "unsupported_callback",
                            "callback parameters must be identifiers",
                            param,
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let body = node
            .get("body")
            .cloned()
            .ok_or_else(|| self.error("parser_error", "callback has no body", node))?;
        Ok(RuntimeValue::Callback(CallbackValue { params, body }))
    }

    fn eval_call(&mut self, node: &JsonValue, depth: usize) -> Result<RuntimeValue, EvalError> {
        let callee = node
            .get("callee")
            .ok_or_else(|| self.error("parser_error", "call has no callee", node))?;
        let arguments = node
            .get("arguments")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| self.error("parser_error", "call has no arguments", node))?;
        let eval_arguments = |evaluator: &mut Self| {
            arguments
                .iter()
                .map(|argument| evaluator.eval(argument, depth + 1))
                .collect::<Result<Vec<_>, _>>()
        };

        let result = match node_type(callee) {
            Some("Identifier") => {
                let name = node_string(callee, "name").unwrap_or("");
                let args = eval_arguments(self)?;
                self.call_global(name, args, node)
            }
            Some("MemberExpression" | "StaticMemberExpression" | "ComputedMemberExpression") => {
                let object_node = callee.get("object").ok_or_else(|| {
                    self.error("parser_error", "method call has no receiver", callee)
                })?;
                if let Some(root) = identifier_name(object_node) {
                    if matches!(
                        root,
                        "Math" | "Number" | "String" | "Array" | "Object" | "JSON" | "Date"
                    ) {
                        let property = self.member_key(callee, depth)?;
                        let args = eval_arguments(self)?;
                        self.call_static(root, &property, args, node)
                    } else {
                        let receiver = self.eval(object_node, depth + 1)?;
                        if matches!(receiver, RuntimeValue::OptionalShortCircuit) {
                            Ok(RuntimeValue::OptionalShortCircuit)
                        } else {
                            let optional = callee
                                .get("optional")
                                .and_then(JsonValue::as_bool)
                                .unwrap_or(false)
                                || node
                                    .get("optional")
                                    .and_then(JsonValue::as_bool)
                                    .unwrap_or(false);
                            if receiver.is_nullish() && optional {
                                Ok(RuntimeValue::OptionalShortCircuit)
                            } else {
                                let property = self.member_key(callee, depth)?;
                                let args = eval_arguments(self)?;
                                self.call_method(receiver, &property, args, node)
                            }
                        }
                    }
                } else {
                    let receiver = self.eval(object_node, depth + 1)?;
                    if matches!(receiver, RuntimeValue::OptionalShortCircuit) {
                        Ok(RuntimeValue::OptionalShortCircuit)
                    } else {
                        let optional = callee
                            .get("optional")
                            .and_then(JsonValue::as_bool)
                            .unwrap_or(false)
                            || node
                                .get("optional")
                                .and_then(JsonValue::as_bool)
                                .unwrap_or(false);
                        if receiver.is_nullish() && optional {
                            Ok(RuntimeValue::OptionalShortCircuit)
                        } else {
                            let property = self.member_key(callee, depth)?;
                            let args = eval_arguments(self)?;
                            self.call_method(receiver, &property, args, node)
                        }
                    }
                }
            }
            _ => Err(self.error(
                "unsupported_call",
                "only allowlisted functions and methods may be called",
                callee,
            )),
        };
        result.map_err(|mut error| {
            if error.span.is_none() {
                error.span = source_span(node, self.source);
            }
            error
        })
    }

    fn member_key(&mut self, node: &JsonValue, depth: usize) -> Result<String, EvalError> {
        let computed = node
            .get("computed")
            .and_then(JsonValue::as_bool)
            .unwrap_or(node_type(node) == Some("ComputedMemberExpression"));
        if computed {
            let property = self.eval_required(node, "property", depth)?;
            property_key(&property)
        } else {
            static_property_name(node.get("property")).ok_or_else(|| {
                self.error("parser_error", "member expression has no property", node)
            })
        }
    }

    fn eval_required(
        &mut self,
        node: &JsonValue,
        key: &str,
        depth: usize,
    ) -> Result<RuntimeValue, EvalError> {
        let child = node
            .get(key)
            .ok_or_else(|| self.error("parser_error", format!("AST node has no `{key}`"), node))?;
        self.eval(child, depth + 1)
    }

    fn call_callback(
        &mut self,
        callback: &CallbackValue,
        arguments: Vec<RuntimeValue>,
    ) -> Result<RuntimeValue, EvalError> {
        self.steps += 1;
        if self.steps > self.limits.max_evaluation_steps {
            return Err(EvalError::new(
                "evaluation_limit",
                format!(
                    "formula exceeds {} evaluation steps",
                    self.limits.max_evaluation_steps
                ),
                None,
            ));
        }
        let scope = callback
            .params
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    name.clone(),
                    arguments
                        .get(index)
                        .cloned()
                        .unwrap_or(RuntimeValue::Undefined),
                )
            })
            .collect();
        self.scopes.push(scope);
        let result = self.eval(&callback.body, 0);
        self.scopes.pop();
        result
    }

    fn callback_arguments(
        &mut self,
        callback: &CallbackValue,
        mut positional: Vec<RuntimeValue>,
        original: Option<&RuntimeValue>,
    ) -> Result<Vec<RuntimeValue>, EvalError> {
        let wanted = callback.params.len();
        positional.truncate(wanted);
        if wanted > positional.len() {
            let original = original.ok_or_else(|| {
                EvalError::new(
                    "unsupported_callback",
                    "callback requests an unavailable collection argument",
                    None,
                )
            })?;
            // RuntimeValue owns its data, so exposing the callback's collection
            // argument requires a clone. Charge that work to the evaluation
            // budget before cloning to keep adversarial callbacks bounded.
            let clone_cost = match original {
                RuntimeValue::List(values) => values.len(),
                RuntimeValue::Object(values) => values.len(),
                _ => 1,
            };
            self.steps = self.steps.saturating_add(clone_cost);
            if self.steps > self.limits.max_evaluation_steps {
                return Err(EvalError::new(
                    "evaluation_limit",
                    format!(
                        "formula exceeds {} evaluation steps",
                        self.limits.max_evaluation_steps
                    ),
                    None,
                ));
            }
            positional.push(original.clone());
        }
        Ok(positional)
    }

    fn step(&mut self, node: &JsonValue) -> Result<(), EvalError> {
        self.steps += 1;
        if self.steps > self.limits.max_evaluation_steps {
            return Err(self.error(
                "evaluation_limit",
                format!(
                    "formula exceeds {} evaluation steps",
                    self.limits.max_evaluation_steps
                ),
                node,
            ));
        }
        Ok(())
    }

    fn ensure_collection(&self, len: usize, node: &JsonValue) -> Result<(), EvalError> {
        if len > self.limits.max_collection_elements {
            Err(self.error(
                "collection_limit",
                format!(
                    "collection exceeds {} elements",
                    self.limits.max_collection_elements
                ),
                node,
            ))
        } else {
            Ok(())
        }
    }

    fn ensure_output_string(&self, value: &str, node: &JsonValue) -> Result<(), EvalError> {
        if value.len() > self.limits.max_serialized_output_bytes {
            Err(self.error(
                "output_limit",
                format!(
                    "string output exceeds {} bytes",
                    self.limits.max_serialized_output_bytes
                ),
                node,
            ))
        } else {
            Ok(())
        }
    }

    fn source_slice(&self, node: &JsonValue) -> Option<&str> {
        let start = node.get("start")?.as_u64()? as usize;
        let end = node.get("end")?.as_u64()? as usize;
        self.source.get(start..end)
    }

    fn error(
        &self,
        code: impl Into<String>,
        message: impl Into<String>,
        node: &JsonValue,
    ) -> EvalError {
        EvalError::new(code, message, source_span(node, self.source))
    }
}

fn identifier_name(node: &JsonValue) -> Option<&str> {
    (node_type(node) == Some("Identifier"))
        .then(|| node_string(node, "name"))
        .flatten()
}

impl Evaluator<'_> {
    fn call_global(
        &mut self,
        name: &str,
        args: Vec<RuntimeValue>,
        node: &JsonValue,
    ) -> Result<RuntimeValue, EvalError> {
        match name {
            "String" => {
                ensure_arity(name, &args, 0, 1)?;
                Ok(RuntimeValue::String(match args.first() {
                    Some(value) => js_string(value)?,
                    None => String::new(),
                }))
            }
            "Number" => {
                ensure_arity(name, &args, 0, 1)?;
                number_conversion(args.first().unwrap_or(&RuntimeValue::Number(Decimal::ZERO)))
                    .map(RuntimeValue::Number)
            }
            "parseInt" => parse_int(&args).map(RuntimeValue::Number),
            "parseFloat" => parse_float(&args).map(RuntimeValue::Number),
            "isFinite" => {
                ensure_arity(name, &args, 1, 1)?;
                Ok(RuntimeValue::Boolean(matches!(
                    args.first(),
                    Some(RuntimeValue::Number(_))
                )))
            }
            "isInteger" => {
                ensure_arity(name, &args, 1, 1)?;
                Ok(RuntimeValue::Boolean(matches!(
                    args.first(),
                    Some(RuntimeValue::Number(value)) if value.is_integer()
                )))
            }
            "encodeURIComponent" => {
                ensure_arity(name, &args, 1, 1)?;
                Ok(RuntimeValue::String(encode_uri_component(&js_string(
                    &args[0],
                )?)))
            }
            "decodeURIComponent" => {
                ensure_arity(name, &args, 1, 1)?;
                decode_uri_component(&js_string(&args[0])?).map(RuntimeValue::String)
            }
            _ => Err(self.error(
                "unsupported_call",
                format!("function `{name}` is not available in formulas"),
                node,
            )),
        }
    }

    fn call_static(
        &mut self,
        root: &str,
        method: &str,
        args: Vec<RuntimeValue>,
        node: &JsonValue,
    ) -> Result<RuntimeValue, EvalError> {
        match root {
            "Math" => call_math(method, &args),
            "Number" => match method {
                "isFinite" => {
                    ensure_arity("Number.isFinite", &args, 1, 1)?;
                    Ok(RuntimeValue::Boolean(matches!(
                        args.first(),
                        Some(RuntimeValue::Number(_))
                    )))
                }
                "isInteger" => {
                    ensure_arity("Number.isInteger", &args, 1, 1)?;
                    Ok(RuntimeValue::Boolean(matches!(
                        args.first(),
                        Some(RuntimeValue::Number(value)) if value.is_integer()
                    )))
                }
                "parseInt" => parse_int(&args).map(RuntimeValue::Number),
                "parseFloat" => parse_float(&args).map(RuntimeValue::Number),
                _ => Err(unsupported_method(root, method)),
            },
            "Array" => match method {
                "isArray" => {
                    ensure_arity("Array.isArray", &args, 1, 1)?;
                    Ok(RuntimeValue::Boolean(matches!(
                        args.first(),
                        Some(RuntimeValue::List(_))
                    )))
                }
                _ => Err(unsupported_method(root, method)),
            },
            "Object" => self.call_object_static(method, args),
            "JSON" => self.call_json_static(method, args),
            "Date" => match method {
                "parse" => {
                    ensure_arity("Date.parse", &args, 1, 1)?;
                    parse_iso_datetime_millis(&js_string(&args[0])?)
                        .map(Decimal::from)
                        .map(RuntimeValue::Number)
                }
                "UTC" => date_utc(&args).map(Decimal::from).map(RuntimeValue::Number),
                _ => Err(unsupported_method(root, method)),
            },
            "String" => Err(unsupported_method(root, method)),
            _ => Err(self.error(
                "unsupported_call",
                format!("global `{root}` is not callable"),
                node,
            )),
        }
    }

    fn call_object_static(
        &mut self,
        method: &str,
        args: Vec<RuntimeValue>,
    ) -> Result<RuntimeValue, EvalError> {
        match method {
            "keys" | "values" | "entries" => {
                ensure_arity(&format!("Object.{method}"), &args, 1, 1)?;
                let entries = enumerable_entries(&args[0])?;
                let values = match method {
                    "keys" => entries
                        .into_iter()
                        .map(|(name, _)| RuntimeValue::String(name))
                        .collect(),
                    "values" => entries.into_iter().map(|(_, value)| value).collect(),
                    "entries" => entries
                        .into_iter()
                        .map(|(name, value)| {
                            RuntimeValue::List(vec![RuntimeValue::String(name), value])
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                self.ensure_runtime_collection(values, &format!("Object.{method}"))
            }
            "hasOwn" => {
                ensure_arity("Object.hasOwn", &args, 2, 2)?;
                let key = property_key(&args[1])?;
                Ok(RuntimeValue::Boolean(has_property(&args[0], &key)))
            }
            _ => Err(unsupported_method("Object", method)),
        }
    }

    fn call_json_static(
        &mut self,
        method: &str,
        args: Vec<RuntimeValue>,
    ) -> Result<RuntimeValue, EvalError> {
        match method {
            "parse" => {
                ensure_arity("JSON.parse", &args, 1, 1)?;
                let source = expect_string(&args[0])?;
                if source.len() > self.limits.max_serialized_output_bytes {
                    return Err(EvalError::new(
                        "input_limit",
                        "JSON input exceeds the formula output budget",
                        None,
                    ));
                }
                let parsed: JsonValue = serde_json::from_str(source).map_err(|error| {
                    EvalError::new("invalid_json", format!("invalid JSON: {error}"), None)
                })?;
                runtime_value_from_json(&parsed, self.limits)
            }
            "stringify" => {
                ensure_arity("JSON.stringify", &args, 1, 1)?;
                enforce_runtime_value_limits(&args[0], self.limits)?;
                let public = args[0].to_public()?;
                let json = public_value_to_json(&public)?;
                let output = serde_json::to_string(&json).map_err(|error| {
                    EvalError::new("serialization_error", error.to_string(), None)
                })?;
                if output.len() > self.limits.max_serialized_output_bytes {
                    return Err(EvalError::new(
                        "output_limit",
                        "JSON output exceeds the formula output budget",
                        None,
                    ));
                }
                Ok(RuntimeValue::String(output))
            }
            _ => Err(unsupported_method("JSON", method)),
        }
    }

    fn call_method(
        &mut self,
        receiver: RuntimeValue,
        method: &str,
        args: Vec<RuntimeValue>,
        node: &JsonValue,
    ) -> Result<RuntimeValue, EvalError> {
        if FORBIDDEN_PROPERTIES.contains(&method) {
            return Err(self.error(
                "forbidden_property",
                format!("property `{method}` is not available in formulas"),
                node,
            ));
        }
        match receiver {
            RuntimeValue::String(value) => self.call_string_method(value, method, args),
            RuntimeValue::List(value) => self.call_array_method(value, method, args),
            RuntimeValue::Regex(value) => self.call_regex_method(value, method, args),
            RuntimeValue::Number(value) => call_number_method(value, method, &args),
            RuntimeValue::Object(_) => Err(unsupported_method("Object", method)),
            RuntimeValue::Null | RuntimeValue::Undefined | RuntimeValue::OptionalShortCircuit => {
                Err(EvalError::new(
                    "null_access",
                    format!("cannot call `{method}` on {}", receiver.type_name()),
                    None,
                ))
            }
            RuntimeValue::Boolean(_) | RuntimeValue::Callback(_) => {
                Err(unsupported_method(receiver.type_name(), method))
            }
        }
    }

    fn call_string_method(
        &mut self,
        value: String,
        method: &str,
        args: Vec<RuntimeValue>,
    ) -> Result<RuntimeValue, EvalError> {
        match method {
            "trim" | "trimStart" | "trimEnd" => {
                ensure_arity(method, &args, 0, 0)?;
                Ok(RuntimeValue::String(
                    match method {
                        "trim" => value.trim(),
                        "trimStart" => value.trim_start(),
                        "trimEnd" => value.trim_end(),
                        _ => &value,
                    }
                    .to_owned(),
                ))
            }
            "toUpperCase" | "toLowerCase" => {
                ensure_arity(method, &args, 0, 0)?;
                Ok(RuntimeValue::String(if method == "toUpperCase" {
                    value.to_uppercase()
                } else {
                    value.to_lowercase()
                }))
            }
            "includes" | "startsWith" | "endsWith" => {
                ensure_arity(method, &args, 1, 2)?;
                let needle = js_string(&args[0])?;
                let haystack = value.chars().collect::<Vec<_>>();
                let needle = needle.chars().collect::<Vec<_>>();
                let requested = args.get(1).map(integer_argument).transpose()?;
                let result = match method {
                    "includes" => {
                        let start = requested.unwrap_or(0).clamp(0, haystack.len() as i64) as usize;
                        needle.is_empty()
                            || haystack[start..]
                                .windows(needle.len())
                                .any(|candidate| candidate == needle)
                    }
                    "startsWith" => {
                        let start = requested.unwrap_or(0).clamp(0, haystack.len() as i64) as usize;
                        haystack
                            .get(start..start.saturating_add(needle.len()))
                            .is_some_and(|candidate| candidate == needle)
                    }
                    "endsWith" => {
                        let end = requested
                            .unwrap_or(haystack.len() as i64)
                            .clamp(0, haystack.len() as i64)
                            as usize;
                        end >= needle.len() && haystack[end - needle.len()..end] == needle
                    }
                    _ => false,
                };
                Ok(RuntimeValue::Boolean(result))
            }
            "charAt" | "at" => {
                ensure_arity(method, &args, 0, 1)?;
                let chars = value.chars().collect::<Vec<_>>();
                let index = args.first().map(integer_argument).transpose()?.unwrap_or(0);
                let index = if method == "at" {
                    normalize_at_index(index, chars.len())
                } else {
                    (index >= 0)
                        .then_some(index as usize)
                        .filter(|index| *index < chars.len())
                };
                Ok(RuntimeValue::String(
                    index
                        .and_then(|index| chars.get(index))
                        .map(char::to_string)
                        .unwrap_or_default(),
                ))
            }
            "slice" | "substring" => {
                ensure_arity(method, &args, 0, 2)?;
                let chars = value.chars().collect::<Vec<_>>();
                let len = chars.len() as i64;
                let start = args.first().map(integer_argument).transpose()?.unwrap_or(0);
                let end = args
                    .get(1)
                    .map(integer_argument)
                    .transpose()?
                    .unwrap_or(len);
                let (start, end) = if method == "slice" {
                    (
                        normalize_slice_index(start, chars.len()),
                        normalize_slice_index(end, chars.len()),
                    )
                } else {
                    let mut start = start.clamp(0, len) as usize;
                    let mut end = end.clamp(0, len) as usize;
                    if start > end {
                        std::mem::swap(&mut start, &mut end);
                    }
                    (start, end)
                };
                Ok(RuntimeValue::String(if end < start {
                    String::new()
                } else {
                    chars[start..end].iter().collect()
                }))
            }
            "indexOf" | "lastIndexOf" => {
                ensure_arity(method, &args, 1, 1)?;
                let needle = js_string(&args[0])?;
                let position = if method == "indexOf" {
                    value.find(&needle)
                } else {
                    value.rfind(&needle)
                };
                let character_position = position.map(|byte| value[..byte].chars().count() as i64);
                Ok(RuntimeValue::Number(Decimal::from(
                    character_position.unwrap_or(-1),
                )))
            }
            "concat" => {
                let mut output = value;
                for argument in args {
                    output.push_str(&js_string(&argument)?);
                }
                if output.len() > self.limits.max_serialized_output_bytes {
                    return Err(EvalError::new(
                        "output_limit",
                        "concatenated string exceeds the formula output budget",
                        None,
                    ));
                }
                Ok(RuntimeValue::String(output))
            }
            "repeat" => {
                ensure_arity(method, &args, 1, 1)?;
                let count = nonnegative_usize(&args[0])?;
                let size = value.len().checked_mul(count).ok_or_else(number_overflow)?;
                if size > self.limits.max_serialized_output_bytes {
                    return Err(EvalError::new(
                        "output_limit",
                        "repeated string exceeds the formula output budget",
                        None,
                    ));
                }
                Ok(RuntimeValue::String(value.repeat(count)))
            }
            "padStart" | "padEnd" => {
                ensure_arity(method, &args, 1, 2)?;
                let target = nonnegative_usize(&args[0])?;
                let current = value.chars().count();
                if target <= current {
                    return Ok(RuntimeValue::String(value));
                }
                let pad = args
                    .get(1)
                    .map(js_string)
                    .transpose()?
                    .unwrap_or_else(|| " ".to_owned());
                if pad.is_empty() {
                    return Ok(RuntimeValue::String(value));
                }
                let needed = target - current;
                // Every Unicode scalar occupies at least one output byte. Reject
                // impossible targets before allocating the padding buffer.
                if target > self.limits.max_serialized_output_bytes {
                    return Err(EvalError::new(
                        "output_limit",
                        "padded string exceeds the formula output budget",
                        None,
                    ));
                }
                let padding = pad.chars().cycle().take(needed).collect::<String>();
                let output = if method == "padStart" {
                    format!("{padding}{value}")
                } else {
                    format!("{value}{padding}")
                };
                if output.len() > self.limits.max_serialized_output_bytes {
                    return Err(EvalError::new(
                        "output_limit",
                        "padded string exceeds the formula output budget",
                        None,
                    ));
                }
                Ok(RuntimeValue::String(output))
            }
            "split" => {
                ensure_arity(method, &args, 0, 2)?;
                let limit = args
                    .get(1)
                    .map(nonnegative_usize)
                    .transpose()?
                    .unwrap_or(self.limits.max_collection_elements)
                    .min(self.limits.max_collection_elements);
                if limit == 0 {
                    return Ok(RuntimeValue::List(Vec::new()));
                }
                let parts = match args.first() {
                    None | Some(RuntimeValue::Undefined | RuntimeValue::OptionalShortCircuit) => {
                        vec![RuntimeValue::String(value)]
                    }
                    Some(RuntimeValue::Regex(regex)) => regex
                        .regex
                        .split(&value)
                        .take(limit)
                        .map(|part| RuntimeValue::String(part.to_owned()))
                        .collect(),
                    Some(separator) => {
                        let separator = js_string(separator)?;
                        if separator.is_empty() {
                            value
                                .chars()
                                .take(limit)
                                .map(|part| RuntimeValue::String(part.to_string()))
                                .collect()
                        } else {
                            value
                                .split(&separator)
                                .take(limit)
                                .map(|part| RuntimeValue::String(part.to_owned()))
                                .collect()
                        }
                    }
                };
                self.ensure_runtime_collection(parts, "String.split")
            }
            "match" => {
                ensure_arity(method, &args, 1, 1)?;
                let RuntimeValue::Regex(regex) = &args[0] else {
                    return Err(EvalError::new(
                        "type_mismatch",
                        "String.match expects a regex literal",
                        None,
                    ));
                };
                let matches = if regex.flags.contains('g') {
                    regex
                        .regex
                        .find_iter(&value)
                        .take(self.limits.max_collection_elements + 1)
                        .map(|capture| RuntimeValue::String(capture.as_str().to_owned()))
                        .collect::<Vec<_>>()
                } else if let Some(captures) = regex.regex.captures(&value) {
                    captures
                        .iter()
                        .map(|capture| {
                            capture
                                .map(|capture| RuntimeValue::String(capture.as_str().to_owned()))
                                .unwrap_or(RuntimeValue::Undefined)
                        })
                        .collect()
                } else {
                    return Ok(RuntimeValue::Null);
                };
                self.ensure_runtime_collection(matches, "String.match")
            }
            "replace" => {
                ensure_arity(method, &args, 2, 2)?;
                let replacement = expect_string(&args[1])?;
                let output = match &args[0] {
                    RuntimeValue::Regex(regex) => {
                        let replacement = js_regex_replacement(replacement);
                        if regex.flags.contains('g') {
                            regex
                                .regex
                                .replace_all(&value, replacement.as_str())
                                .into_owned()
                        } else {
                            regex
                                .regex
                                .replace(&value, replacement.as_str())
                                .into_owned()
                        }
                    }
                    search => value.replacen(&js_string(search)?, replacement, 1),
                };
                if output.len() > self.limits.max_serialized_output_bytes {
                    return Err(EvalError::new(
                        "output_limit",
                        "replacement output exceeds the formula output budget",
                        None,
                    ));
                }
                Ok(RuntimeValue::String(output))
            }
            _ => Err(unsupported_method("String", method)),
        }
    }

    fn ensure_runtime_collection(
        &self,
        values: Vec<RuntimeValue>,
        operation: &str,
    ) -> Result<RuntimeValue, EvalError> {
        if values.len() > self.limits.max_collection_elements {
            Err(EvalError::new(
                "collection_limit",
                format!(
                    "{operation} exceeds {} collection elements",
                    self.limits.max_collection_elements
                ),
                None,
            ))
        } else {
            let value = RuntimeValue::List(values);
            enforce_runtime_value_limits(&value, self.limits).map_err(|mut error| {
                error.message = format!("{operation}: {}", error.message);
                error
            })?;
            Ok(value)
        }
    }
}

impl Evaluator<'_> {
    fn call_array_method(
        &mut self,
        values: Vec<RuntimeValue>,
        method: &str,
        args: Vec<RuntimeValue>,
    ) -> Result<RuntimeValue, EvalError> {
        match method {
            "at" => {
                ensure_arity(method, &args, 1, 1)?;
                let index = integer_argument(&args[0])?;
                Ok(normalize_at_index(index, values.len())
                    .and_then(|index| values.get(index).cloned())
                    .unwrap_or(RuntimeValue::Undefined))
            }
            "includes" => {
                ensure_arity(method, &args, 1, 2)?;
                let start = args
                    .get(1)
                    .map(integer_argument)
                    .transpose()?
                    .map(|index| normalize_slice_index(index, values.len()))
                    .unwrap_or(0);
                Ok(RuntimeValue::Boolean(
                    values[start..]
                        .iter()
                        .any(|value| strict_equal(value, &args[0])),
                ))
            }
            "indexOf" | "lastIndexOf" => {
                ensure_arity(method, &args, 1, 1)?;
                let position = if method == "indexOf" {
                    values
                        .iter()
                        .position(|value| strict_equal(value, &args[0]))
                } else {
                    values
                        .iter()
                        .rposition(|value| strict_equal(value, &args[0]))
                };
                Ok(RuntimeValue::Number(Decimal::from(
                    position.map(|index| index as i64).unwrap_or(-1),
                )))
            }
            "join" => {
                ensure_arity(method, &args, 0, 1)?;
                let separator = args
                    .first()
                    .map(js_string)
                    .transpose()?
                    .unwrap_or_else(|| ",".to_owned());
                let mut output = String::new();
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push_str(&separator);
                    }
                    if !value.is_nullish() {
                        output.push_str(&js_string(value)?);
                    }
                    if output.len() > self.limits.max_serialized_output_bytes {
                        return Err(EvalError::new(
                            "output_limit",
                            "joined string exceeds the formula output budget",
                            None,
                        ));
                    }
                }
                Ok(RuntimeValue::String(output))
            }
            "slice" => {
                ensure_arity(method, &args, 0, 2)?;
                let start = args
                    .first()
                    .map(integer_argument)
                    .transpose()?
                    .map(|index| normalize_slice_index(index, values.len()))
                    .unwrap_or(0);
                let end = args
                    .get(1)
                    .map(integer_argument)
                    .transpose()?
                    .map(|index| normalize_slice_index(index, values.len()))
                    .unwrap_or(values.len());
                self.ensure_runtime_collection(
                    if end < start {
                        Vec::new()
                    } else {
                        values[start..end].to_vec()
                    },
                    "Array.slice",
                )
            }
            "concat" => {
                let mut output = values;
                for argument in args {
                    match argument {
                        RuntimeValue::List(mut nested) => output.append(&mut nested),
                        value => output.push(value),
                    }
                    if output.len() > self.limits.max_collection_elements {
                        return Err(EvalError::new(
                            "collection_limit",
                            "Array.concat exceeds the collection budget",
                            None,
                        ));
                    }
                }
                self.ensure_runtime_collection(output, "Array.concat")
            }
            "flat" => {
                ensure_arity(method, &args, 0, 1)?;
                let depth = args
                    .first()
                    .map(nonnegative_usize)
                    .transpose()?
                    .unwrap_or(1);
                if depth > self.limits.max_nesting_depth {
                    return Err(EvalError::new(
                        "nesting_limit",
                        format!("Array.flat depth exceeds {}", self.limits.max_nesting_depth),
                        None,
                    ));
                }
                let mut output = Vec::new();
                flatten_values(
                    &values,
                    depth,
                    &mut output,
                    self.limits.max_collection_elements,
                )?;
                Ok(RuntimeValue::List(output))
            }
            "map" | "filter" | "some" | "every" | "find" => {
                ensure_arity(method, &args, 1, 1)?;
                let callback = expect_callback(&args[0])?.clone();
                let original =
                    (callback.params.len() > 2).then(|| RuntimeValue::List(values.clone()));
                match method {
                    "map" => {
                        let mut output = Vec::with_capacity(values.len());
                        let mut total_elements = values.len();
                        let mut text_bytes = 0usize;
                        for (index, value) in values.into_iter().enumerate() {
                            let arguments = self.callback_arguments(
                                &callback,
                                vec![value, RuntimeValue::Number(Decimal::from(index as u64))],
                                original.as_ref(),
                            )?;
                            let mapped = self.call_callback(&callback, arguments)?;
                            let (nested_elements, nested_text) =
                                runtime_value_usage(&mapped, self.limits)?;
                            total_elements = total_elements.saturating_add(nested_elements);
                            text_bytes = text_bytes.saturating_add(nested_text);
                            if total_elements > self.limits.max_collection_elements {
                                return Err(EvalError::new(
                                    "collection_limit",
                                    "Array.map exceeds the total collection budget",
                                    None,
                                ));
                            }
                            if text_bytes > self.limits.max_serialized_output_bytes {
                                return Err(EvalError::new(
                                    "output_limit",
                                    "Array.map exceeds the serialized output budget",
                                    None,
                                ));
                            }
                            output.push(mapped);
                        }
                        self.ensure_runtime_collection(output, "Array.map")
                    }
                    "filter" => {
                        let mut output = Vec::new();
                        for (index, value) in values.into_iter().enumerate() {
                            let arguments = self.callback_arguments(
                                &callback,
                                vec![
                                    value.clone(),
                                    RuntimeValue::Number(Decimal::from(index as u64)),
                                ],
                                original.as_ref(),
                            )?;
                            let keep = self.call_callback(&callback, arguments)?;
                            if keep.truthy() {
                                output.push(value);
                            }
                        }
                        self.ensure_runtime_collection(output, "Array.filter")
                    }
                    "some" => {
                        for (index, value) in values.into_iter().enumerate() {
                            let arguments = self.callback_arguments(
                                &callback,
                                vec![value, RuntimeValue::Number(Decimal::from(index as u64))],
                                original.as_ref(),
                            )?;
                            if self.call_callback(&callback, arguments)?.truthy() {
                                return Ok(RuntimeValue::Boolean(true));
                            }
                        }
                        Ok(RuntimeValue::Boolean(false))
                    }
                    "every" => {
                        for (index, value) in values.into_iter().enumerate() {
                            let arguments = self.callback_arguments(
                                &callback,
                                vec![value, RuntimeValue::Number(Decimal::from(index as u64))],
                                original.as_ref(),
                            )?;
                            if !self.call_callback(&callback, arguments)?.truthy() {
                                return Ok(RuntimeValue::Boolean(false));
                            }
                        }
                        Ok(RuntimeValue::Boolean(true))
                    }
                    "find" => {
                        for (index, value) in values.into_iter().enumerate() {
                            let arguments = self.callback_arguments(
                                &callback,
                                vec![
                                    value.clone(),
                                    RuntimeValue::Number(Decimal::from(index as u64)),
                                ],
                                original.as_ref(),
                            )?;
                            if self.call_callback(&callback, arguments)?.truthy() {
                                return Ok(value);
                            }
                        }
                        Ok(RuntimeValue::Undefined)
                    }
                    _ => unreachable!(),
                }
            }
            "reduce" => {
                ensure_arity(method, &args, 1, 2)?;
                let callback = expect_callback(&args[0])?.clone();
                if values.is_empty() && args.len() == 1 {
                    return Err(EvalError::new(
                        "empty_reduce",
                        "cannot reduce an empty array without an initial value",
                        None,
                    ));
                }
                let original =
                    (callback.params.len() > 3).then(|| RuntimeValue::List(values.clone()));
                let (mut accumulator, start) = if let Some(initial) = args.get(1) {
                    (initial.clone(), 0)
                } else {
                    (values[0].clone(), 1)
                };
                for (index, value) in values.into_iter().enumerate().skip(start) {
                    let arguments = self.callback_arguments(
                        &callback,
                        vec![
                            accumulator,
                            value,
                            RuntimeValue::Number(Decimal::from(index as u64)),
                        ],
                        original.as_ref(),
                    )?;
                    accumulator = self.call_callback(&callback, arguments)?;
                    enforce_runtime_value_limits(&accumulator, self.limits)?;
                }
                Ok(accumulator)
            }
            _ => Err(unsupported_method("Array", method)),
        }
    }

    fn call_regex_method(
        &self,
        regex: RegexValue,
        method: &str,
        args: Vec<RuntimeValue>,
    ) -> Result<RuntimeValue, EvalError> {
        match method {
            "test" => {
                ensure_arity(method, &args, 1, 1)?;
                Ok(RuntimeValue::Boolean(
                    regex.regex.is_match(&js_string(&args[0])?),
                ))
            }
            _ => Err(unsupported_method("RegExp", method)),
        }
    }
}

fn call_math(method: &str, args: &[RuntimeValue]) -> Result<RuntimeValue, EvalError> {
    match method {
        "abs" | "ceil" | "floor" | "round" | "trunc" | "sqrt" | "sign" | "exp" | "log"
        | "log10" => {
            ensure_arity(&format!("Math.{method}"), args, 1, 1)?;
            let number = expect_number(&args[0])?;
            let result = match method {
                "abs" => Some(number.abs()),
                "ceil" => Some(number.ceil()),
                "floor" => Some(number.floor()),
                "round" => Some(
                    number
                        .checked_add(Decimal::new(5, 1))
                        .ok_or_else(number_overflow)?
                        .floor(),
                ),
                "trunc" => Some(number.trunc()),
                "sqrt" => number.sqrt(),
                "sign" => Some(if number.is_zero() {
                    Decimal::ZERO
                } else if number.is_sign_negative() {
                    Decimal::NEGATIVE_ONE
                } else {
                    Decimal::ONE
                }),
                "exp" => number.checked_exp(),
                "log" => number.checked_ln(),
                "log10" => number.checked_log10(),
                _ => None,
            };
            result.map(RuntimeValue::Number).ok_or_else(|| {
                EvalError::new(
                    "math_domain",
                    format!("Math.{method} is undefined for `{number}`"),
                    None,
                )
            })
        }
        "min" | "max" => {
            ensure_arity(&format!("Math.{method}"), args, 1, usize::MAX)?;
            let mut numbers = args.iter().map(expect_number);
            let mut result = numbers.next().transpose()?.ok_or_else(|| {
                EvalError::new("arity", format!("Math.{method} needs an argument"), None)
            })?;
            for number in numbers {
                let number = number?;
                if (method == "min" && number < result) || (method == "max" && number > result) {
                    result = number;
                }
            }
            Ok(RuntimeValue::Number(result))
        }
        "pow" => {
            ensure_arity("Math.pow", args, 2, 2)?;
            let base = expect_number(&args[0])?;
            let exponent = expect_number(&args[1])?;
            base.checked_powd(exponent)
                .map(RuntimeValue::Number)
                .ok_or_else(number_overflow)
        }
        _ => Err(unsupported_method("Math", method)),
    }
}

fn call_number_method(
    value: Decimal,
    method: &str,
    args: &[RuntimeValue],
) -> Result<RuntimeValue, EvalError> {
    match method {
        "toString" => {
            ensure_arity(method, args, 0, 0)?;
            Ok(RuntimeValue::String(value.normalize().to_string()))
        }
        "toFixed" => {
            ensure_arity(method, args, 0, 1)?;
            let digits = args
                .first()
                .map(nonnegative_usize)
                .transpose()?
                .unwrap_or(0);
            if digits > 28 {
                return Err(EvalError::new(
                    "number_overflow",
                    "toFixed supports at most 28 decimal places",
                    None,
                ));
            }
            let rounded =
                value.round_dp_with_strategy(digits as u32, RoundingStrategy::MidpointAwayFromZero);
            Ok(RuntimeValue::String(format!("{rounded:.digits$}")))
        }
        _ => Err(unsupported_method("Number", method)),
    }
}

fn validate_result(
    value: &RuntimeValue,
    result_type: FormulaResultType,
) -> Result<FormulaValue, EvalError> {
    if matches!(value, RuntimeValue::Null) {
        return Ok(FormulaValue::Null);
    }
    if matches!(
        value,
        RuntimeValue::Undefined | RuntimeValue::OptionalShortCircuit
    ) {
        return Err(EvalError::new(
            "undefined_result",
            "formula returned undefined",
            None,
        ));
    }
    let valid = match result_type {
        FormulaResultType::String => matches!(value, RuntimeValue::String(_)),
        FormulaResultType::Number => matches!(value, RuntimeValue::Number(_)),
        FormulaResultType::Boolean => matches!(value, RuntimeValue::Boolean(_)),
        FormulaResultType::Date => {
            matches!(&value, RuntimeValue::String(value) if parse_iso_date(value).is_ok())
        }
        FormulaResultType::DateTime => {
            matches!(&value, RuntimeValue::String(value)
                if value.len() > 10 && parse_iso_datetime_millis(value).is_ok())
        }
        FormulaResultType::List => matches!(value, RuntimeValue::List(_)),
        FormulaResultType::Json => !matches!(
            value,
            RuntimeValue::Undefined
                | RuntimeValue::OptionalShortCircuit
                | RuntimeValue::Regex(_)
                | RuntimeValue::Callback(_)
        ),
    };
    if !valid {
        return Err(EvalError::new(
            "result_type",
            format!(
                "expected a {result_type} result, but formula returned {}",
                value.type_name()
            ),
            None,
        ));
    }
    value.to_public()
}

fn enforce_runtime_value_limits(
    value: &RuntimeValue,
    limits: &FormulaLimits,
) -> Result<(), EvalError> {
    runtime_value_usage(value, limits).map(|_| ())
}

fn runtime_value_usage(
    value: &RuntimeValue,
    limits: &FormulaLimits,
) -> Result<(usize, usize), EvalError> {
    fn visit(
        value: &RuntimeValue,
        limits: &FormulaLimits,
        depth: usize,
        elements: &mut usize,
        text_bytes: &mut usize,
    ) -> Result<(), EvalError> {
        if depth > limits.max_nesting_depth {
            return Err(EvalError::new(
                "nesting_limit",
                format!(
                    "formula value exceeds nesting depth {}",
                    limits.max_nesting_depth
                ),
                None,
            ));
        }
        match value {
            RuntimeValue::String(value) => {
                *text_bytes = text_bytes.saturating_add(value.len());
            }
            RuntimeValue::List(values) => {
                *elements = elements.saturating_add(values.len());
                for value in values {
                    visit(value, limits, depth + 1, elements, text_bytes)?;
                }
            }
            RuntimeValue::Object(values) => {
                *elements = elements.saturating_add(values.len());
                for (name, value) in values {
                    *text_bytes = text_bytes.saturating_add(name.len());
                    visit(value, limits, depth + 1, elements, text_bytes)?;
                }
            }
            _ => {}
        }
        if *elements > limits.max_collection_elements {
            return Err(EvalError::new(
                "collection_limit",
                format!(
                    "formula value exceeds {} total collection elements",
                    limits.max_collection_elements
                ),
                None,
            ));
        }
        if *text_bytes > limits.max_serialized_output_bytes {
            return Err(EvalError::new(
                "output_limit",
                "formula value exceeds the serialized output budget",
                None,
            ));
        }
        Ok(())
    }

    let mut elements = 0;
    let mut text_bytes = 0;
    visit(value, limits, 0, &mut elements, &mut text_bytes)?;
    Ok((elements, text_bytes))
}

fn expect_number(value: &RuntimeValue) -> Result<Decimal, EvalError> {
    if let RuntimeValue::Number(value) = value {
        Ok(*value)
    } else {
        Err(EvalError::new(
            "type_mismatch",
            format!("expected number, got {}", value.type_name()),
            None,
        ))
    }
}

fn expect_string(value: &RuntimeValue) -> Result<&str, EvalError> {
    if let RuntimeValue::String(value) = value {
        Ok(value)
    } else {
        Err(EvalError::new(
            "type_mismatch",
            format!("expected string, got {}", value.type_name()),
            None,
        ))
    }
}

fn expect_callback(value: &RuntimeValue) -> Result<&CallbackValue, EvalError> {
    if let RuntimeValue::Callback(value) = value {
        Ok(value)
    } else {
        Err(EvalError::new(
            "type_mismatch",
            format!("expected arrow callback, got {}", value.type_name()),
            None,
        ))
    }
}

fn number_conversion(value: &RuntimeValue) -> Result<Decimal, EvalError> {
    match value {
        RuntimeValue::Number(value) => Ok(*value),
        RuntimeValue::Null => Ok(Decimal::ZERO),
        RuntimeValue::Boolean(value) => Ok(if *value { Decimal::ONE } else { Decimal::ZERO }),
        RuntimeValue::String(value) if value.trim().is_empty() => Ok(Decimal::ZERO),
        RuntimeValue::String(value) => decimal_from_str(value.trim()).map_err(|_| {
            EvalError::new(
                "invalid_number",
                format!("`{value}` is not a supported finite decimal"),
                None,
            )
        }),
        RuntimeValue::List(values) if values.is_empty() => Ok(Decimal::ZERO),
        RuntimeValue::List(values) if values.len() == 1 => {
            number_conversion(&RuntimeValue::String(js_string(&values[0])?))
        }
        _ => Err(EvalError::new(
            "invalid_number",
            format!("cannot convert {} to a number", value.type_name()),
            None,
        )),
    }
}

fn integer_argument(value: &RuntimeValue) -> Result<i64, EvalError> {
    number_conversion(value)?
        .trunc()
        .to_i64()
        .ok_or_else(number_overflow)
}

fn nonnegative_usize(value: &RuntimeValue) -> Result<usize, EvalError> {
    let number = integer_argument(value)?;
    usize::try_from(number)
        .map_err(|_| EvalError::new("range_error", "expected a non-negative integer", None))
}

fn checked_numeric_binary(
    left: RuntimeValue,
    right: RuntimeValue,
    operation: fn(Decimal, Decimal) -> Option<Decimal>,
) -> Result<RuntimeValue, EvalError> {
    let left = expect_number(&left)?;
    let right = expect_number(&right)?;
    operation(left, right)
        .map(RuntimeValue::Number)
        .ok_or_else(number_overflow)
}

fn add_values(left: RuntimeValue, right: RuntimeValue) -> Result<RuntimeValue, EvalError> {
    if matches!(left, RuntimeValue::String(_)) || matches!(right, RuntimeValue::String(_)) {
        return Ok(RuntimeValue::String(format!(
            "{}{}",
            js_string(&left)?,
            js_string(&right)?
        )));
    }
    checked_numeric_binary(left, right, Decimal::checked_add)
}

fn number_overflow() -> EvalError {
    EvalError::new(
        "number_overflow",
        "decimal arithmetic overflowed the supported range",
        None,
    )
}

fn strict_equal(left: &RuntimeValue, right: &RuntimeValue) -> bool {
    match (left, right) {
        (
            RuntimeValue::Undefined | RuntimeValue::OptionalShortCircuit,
            RuntimeValue::Undefined | RuntimeValue::OptionalShortCircuit,
        )
        | (RuntimeValue::Null, RuntimeValue::Null) => true,
        (RuntimeValue::Boolean(left), RuntimeValue::Boolean(right)) => left == right,
        (RuntimeValue::Number(left), RuntimeValue::Number(right)) => left == right,
        (RuntimeValue::String(left), RuntimeValue::String(right)) => left == right,
        // JavaScript compares arrays/objects by identity. Formula values have no
        // mutable identity, so distinct structured values are never strictly equal.
        _ => false,
    }
}

fn compare_values(left: &RuntimeValue, right: &RuntimeValue) -> Result<Ordering, EvalError> {
    match (left, right) {
        (RuntimeValue::Number(left), RuntimeValue::Number(right)) => Ok(left.cmp(right)),
        (RuntimeValue::String(left), RuntimeValue::String(right)) => Ok(left.cmp(right)),
        _ => Err(EvalError::new(
            "type_mismatch",
            format!(
                "cannot compare {} with {}",
                left.type_name(),
                right.type_name()
            ),
            None,
        )),
    }
}

fn js_string(value: &RuntimeValue) -> Result<String, EvalError> {
    match value {
        RuntimeValue::Undefined | RuntimeValue::OptionalShortCircuit => Ok("undefined".to_owned()),
        RuntimeValue::Null => Ok("null".to_owned()),
        RuntimeValue::Boolean(value) => Ok(value.to_string()),
        RuntimeValue::Number(value) => Ok(value.normalize().to_string()),
        RuntimeValue::String(value) => Ok(value.clone()),
        RuntimeValue::List(values) => values
            .iter()
            .map(|value| {
                if value.is_nullish() {
                    Ok(String::new())
                } else {
                    js_string(value)
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|values| values.join(",")),
        RuntimeValue::Object(_) => Ok("[object Object]".to_owned()),
        RuntimeValue::Regex(regex) => Ok(format!("/{}/{}", regex.pattern, regex.flags)),
        RuntimeValue::Callback(_) => Err(EvalError::new(
            "type_mismatch",
            "callbacks cannot be converted to strings",
            None,
        )),
    }
}

fn property_key(value: &RuntimeValue) -> Result<String, EvalError> {
    let key = js_string(value)?;
    if FORBIDDEN_PROPERTIES.contains(&key.as_str()) {
        return Err(EvalError::new(
            "forbidden_property",
            format!("property `{key}` is not available in formulas"),
            None,
        ));
    }
    Ok(key)
}

fn get_property(
    object: &RuntimeValue,
    key: &str,
    optional: bool,
) -> Result<RuntimeValue, EvalError> {
    if FORBIDDEN_PROPERTIES.contains(&key) {
        return Err(EvalError::new(
            "forbidden_property",
            format!("property `{key}` is not available in formulas"),
            None,
        ));
    }
    match object {
        RuntimeValue::Object(values) => {
            Ok(values.get(key).cloned().unwrap_or(RuntimeValue::Undefined))
        }
        RuntimeValue::List(values) => {
            if key == "length" {
                return Ok(RuntimeValue::Number(Decimal::from(values.len() as u64)));
            }
            let index = key.parse::<usize>().ok();
            Ok(index
                .and_then(|index| values.get(index).cloned())
                .unwrap_or(RuntimeValue::Undefined))
        }
        RuntimeValue::String(value) => {
            if key == "length" {
                return Ok(RuntimeValue::Number(Decimal::from(
                    value.chars().count() as u64
                )));
            }
            let index = key.parse::<usize>().ok();
            Ok(index
                .and_then(|index| value.chars().nth(index))
                .map(|value| RuntimeValue::String(value.to_string()))
                .unwrap_or(RuntimeValue::Undefined))
        }
        RuntimeValue::OptionalShortCircuit => Ok(RuntimeValue::OptionalShortCircuit),
        RuntimeValue::Null | RuntimeValue::Undefined if optional => {
            Ok(RuntimeValue::OptionalShortCircuit)
        }
        RuntimeValue::Null | RuntimeValue::Undefined => Err(EvalError::new(
            "null_access",
            format!("cannot read `{key}` from {}", object.type_name()),
            None,
        )),
        _ => Ok(RuntimeValue::Undefined),
    }
}

fn has_property(object: &RuntimeValue, key: &str) -> bool {
    match object {
        RuntimeValue::Object(values) => values.contains_key(key),
        RuntimeValue::List(values) => {
            key == "length" || key.parse::<usize>().is_ok_and(|index| index < values.len())
        }
        RuntimeValue::String(value) => {
            key == "length"
                || key
                    .parse::<usize>()
                    .is_ok_and(|index| index < value.chars().count())
        }
        _ => false,
    }
}

fn enumerable_entries(value: &RuntimeValue) -> Result<Vec<(String, RuntimeValue)>, EvalError> {
    match value {
        RuntimeValue::Object(values) => Ok(values
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()),
        RuntimeValue::List(values) => Ok(values
            .iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value.clone()))
            .collect()),
        RuntimeValue::String(value) => Ok(value
            .chars()
            .enumerate()
            .map(|(index, value)| (index.to_string(), RuntimeValue::String(value.to_string())))
            .collect()),
        RuntimeValue::Null | RuntimeValue::Undefined | RuntimeValue::OptionalShortCircuit => {
            Err(EvalError::new(
                "type_mismatch",
                "Object operation cannot read null or undefined",
                None,
            ))
        }
        _ => Ok(Vec::new()),
    }
}

fn ensure_arity(
    name: &str,
    args: &[RuntimeValue],
    min: usize,
    max: usize,
) -> Result<(), EvalError> {
    if args.len() < min || args.len() > max {
        let expected = if min == max {
            min.to_string()
        } else if max == usize::MAX {
            format!("at least {min}")
        } else {
            format!("{min} to {max}")
        };
        Err(EvalError::new(
            "arity",
            format!("{name} expects {expected} argument(s), got {}", args.len()),
            None,
        ))
    } else {
        Ok(())
    }
}

fn unsupported_method(root: &str, method: &str) -> EvalError {
    EvalError::new(
        "unsupported_call",
        format!("`{root}.{method}` is not available in formulas"),
        None,
    )
}

fn normalize_at_index(index: i64, len: usize) -> Option<usize> {
    let len = len as i64;
    let index = if index < 0 { len + index } else { index };
    (index >= 0 && index < len).then_some(index as usize)
}

fn normalize_slice_index(index: i64, len: usize) -> usize {
    let len = len as i64;
    if index < 0 {
        (len + index).max(0) as usize
    } else {
        index.min(len) as usize
    }
}

fn flatten_values(
    values: &[RuntimeValue],
    depth: usize,
    output: &mut Vec<RuntimeValue>,
    limit: usize,
) -> Result<(), EvalError> {
    for value in values {
        if depth > 0 {
            if let RuntimeValue::List(values) = value {
                flatten_values(values, depth - 1, output, limit)?;
                continue;
            }
        }
        output.push(value.clone());
        if output.len() > limit {
            return Err(EvalError::new(
                "collection_limit",
                "Array.flat exceeds the collection budget",
                None,
            ));
        }
    }
    Ok(())
}

fn parse_int(args: &[RuntimeValue]) -> Result<Decimal, EvalError> {
    ensure_arity("parseInt", args, 1, 2)?;
    let source = js_string(&args[0])?;
    let mut source = source.trim_start();
    let negative = source.starts_with('-');
    if source.starts_with(['-', '+']) {
        source = &source[1..];
    }
    let radix = args
        .get(1)
        .map(integer_argument)
        .transpose()?
        .unwrap_or_else(|| {
            if source.starts_with("0x") || source.starts_with("0X") {
                16
            } else {
                10
            }
        });
    if !(2..=36).contains(&radix) {
        return Err(EvalError::new(
            "range_error",
            "parseInt radix must be between 2 and 36",
            None,
        ));
    }
    if radix == 16 && (source.starts_with("0x") || source.starts_with("0X")) {
        source = &source[2..];
    }
    let digits = source
        .chars()
        .take_while(|character| character.is_digit(radix as u32))
        .collect::<String>();
    if digits.is_empty() {
        return Err(EvalError::new(
            "invalid_number",
            "parseInt did not find an integer",
            None,
        ));
    }
    let mut value =
        Decimal::from_str_radix(&digits, radix as u32).map_err(|_| number_overflow())?;
    if negative {
        value = Decimal::ZERO
            .checked_sub(value)
            .ok_or_else(number_overflow)?;
    }
    Ok(value)
}

fn parse_float(args: &[RuntimeValue]) -> Result<Decimal, EvalError> {
    ensure_arity("parseFloat", args, 1, 1)?;
    let source = js_string(&args[0])?;
    let source = source.trim_start();
    let bytes = source.as_bytes();
    let mut index = 0;
    if bytes
        .first()
        .is_some_and(|byte| matches!(byte, b'+' | b'-'))
    {
        index += 1;
    }
    let integer_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let mut has_digit = index > integer_start;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        has_digit |= index > fraction_start;
    }
    if !has_digit {
        return Err(EvalError::new(
            "invalid_number",
            "parseFloat did not find a decimal number",
            None,
        ));
    }
    if bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b'e' | b'E'))
    {
        let exponent_marker = index;
        index += 1;
        if bytes
            .get(index)
            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        {
            index += 1;
        }
        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent_start {
            index = exponent_marker;
        }
    }
    decimal_from_str(&source[..index]).map_err(|_| number_overflow())
}

fn compile_regex(
    pattern: &str,
    flags: &str,
    limits: &FormulaLimits,
    span: Option<FormulaSourceSpan>,
) -> Result<RegexValue, EvalError> {
    if pattern.len() > limits.max_regex_source_bytes {
        return Err(EvalError::new(
            "regex_limit",
            format!(
                "regex source exceeds {} bytes",
                limits.max_regex_source_bytes
            ),
            span,
        ));
    }
    let mut seen = BTreeSet::new();
    for flag in flags.chars() {
        if !seen.insert(flag) {
            return Err(EvalError::new(
                "invalid_regex",
                format!("duplicate regex flag `{flag}`"),
                span,
            ));
        }
        if !matches!(flag, 'g' | 'i' | 'm' | 's' | 'u') {
            return Err(EvalError::new(
                "invalid_regex",
                format!("regex flag `{flag}` is not supported"),
                span,
            ));
        }
    }
    let mut builder = RegexBuilder::new(pattern);
    builder
        .case_insensitive(flags.contains('i'))
        .multi_line(flags.contains('m'))
        .dot_matches_new_line(flags.contains('s'))
        .unicode(true)
        .size_limit(limits.max_regex_compiled_bytes)
        .dfa_size_limit(limits.max_regex_compiled_bytes);
    let regex = builder.build().map_err(|error| {
        EvalError::new(
            "invalid_regex",
            format!("regex is not supported: {error}"),
            span,
        )
    })?;
    Ok(RegexValue {
        pattern: pattern.to_owned(),
        flags: flags.to_owned(),
        regex,
    })
}

fn js_regex_replacement(replacement: &str) -> String {
    let mut output = String::with_capacity(replacement.len());
    let chars = replacement.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '$' && chars.get(index + 1) == Some(&'&') {
            output.push_str("${0}");
            index += 2;
        } else {
            output.push(chars[index]);
            index += 1;
        }
    }
    output
}

fn encode_uri_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        let character = char::from(*byte);
        if character.is_ascii_alphanumeric()
            || matches!(
                character,
                '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')'
            )
        {
            output.push(character);
        } else {
            output.push('%');
            output.push_str(&format!("{byte:02X}"));
        }
    }
    output
}

fn decode_uri_component(value: &str) -> Result<String, EvalError> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).and_then(|value| hex_value(*value));
            let low = bytes.get(index + 2).and_then(|value| hex_value(*value));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(EvalError::new(
                    "invalid_uri",
                    "URI component contains an invalid percent escape",
                    None,
                ));
            };
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).map_err(|_| {
        EvalError::new(
            "invalid_uri",
            "URI component does not decode to valid UTF-8",
            None,
        )
    })
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn parse_iso_date(value: &str) -> Result<(i64, u32, u32), EvalError> {
    if value.len() != 10
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
    {
        return Err(EvalError::new(
            "invalid_date",
            "date must use YYYY-MM-DD",
            None,
        ));
    }
    let year = value[0..4].parse::<i64>().map_err(|_| invalid_date())?;
    let month = value[5..7].parse::<u32>().map_err(|_| invalid_date())?;
    let day = value[8..10].parse::<u32>().map_err(|_| invalid_date())?;
    validate_date(year, month, day)?;
    Ok((year, month, day))
}

fn parse_iso_datetime_millis(value: &str) -> Result<i64, EvalError> {
    if value.len() == 10 {
        let (year, month, day) = parse_iso_date(value)?;
        return days_from_civil(year, month, day)
            .checked_mul(86_400_000)
            .ok_or_else(number_overflow);
    }
    if value.len() < 20 || value.as_bytes().get(10) != Some(&b'T') {
        return Err(EvalError::new(
            "invalid_datetime",
            "datetime must be ISO 8601 with an explicit timezone",
            None,
        ));
    }
    let (year, month, day) = parse_iso_date(&value[..10])?;
    let hour = parse_two_digits(value, 11)?;
    if value.as_bytes().get(13) != Some(&b':') {
        return Err(invalid_datetime());
    }
    let minute = parse_two_digits(value, 14)?;
    if value.as_bytes().get(16) != Some(&b':') {
        return Err(invalid_datetime());
    }
    let second = parse_two_digits(value, 17)?;
    if hour > 23 || minute > 59 || second > 59 {
        return Err(invalid_datetime());
    }
    let mut cursor = 19;
    let mut millis = 0_u32;
    if value.as_bytes().get(cursor) == Some(&b'.') {
        cursor += 1;
        let start = cursor;
        while value.as_bytes().get(cursor).is_some_and(u8::is_ascii_digit) && cursor - start < 9 {
            cursor += 1;
        }
        if cursor == start {
            return Err(invalid_datetime());
        }
        let fraction = &value[start..cursor];
        let first_three = &fraction[..fraction.len().min(3)];
        millis = first_three.parse::<u32>().map_err(|_| invalid_datetime())?
            * 10_u32.pow((3 - first_three.len()) as u32);
    }
    let offset_minutes = if value.as_bytes().get(cursor) == Some(&b'Z') && cursor + 1 == value.len()
    {
        0_i64
    } else {
        let sign = match value.as_bytes().get(cursor) {
            Some(b'+') => 1_i64,
            Some(b'-') => -1_i64,
            _ => return Err(invalid_datetime()),
        };
        if value.as_bytes().get(cursor + 3) != Some(&b':') || cursor + 6 != value.len() {
            return Err(invalid_datetime());
        }
        let offset_hour = parse_two_digits(value, cursor + 1)?;
        let offset_minute = parse_two_digits(value, cursor + 4)?;
        if offset_hour > 23 || offset_minute > 59 {
            return Err(invalid_datetime());
        }
        sign * i64::from(offset_hour * 60 + offset_minute)
    };
    let days = days_from_civil(year, month, day);
    days.checked_mul(86_400_000)
        .and_then(|value| value.checked_add(i64::from(hour) * 3_600_000))
        .and_then(|value| value.checked_add(i64::from(minute) * 60_000))
        .and_then(|value| value.checked_add(i64::from(second) * 1_000))
        .and_then(|value| value.checked_add(i64::from(millis)))
        .and_then(|value| value.checked_sub(offset_minutes * 60_000))
        .ok_or_else(number_overflow)
}

fn date_utc(args: &[RuntimeValue]) -> Result<i64, EvalError> {
    ensure_arity("Date.UTC", args, 2, 7)?;
    let mut year = integer_argument(&args[0])?;
    if (0..=99).contains(&year) {
        year += 1900;
    }
    let month = integer_argument(&args[1])?;
    let normalized_year = year
        .checked_add(month.div_euclid(12))
        .ok_or_else(number_overflow)?;
    let normalized_month = month.rem_euclid(12) as u32 + 1;
    let day = args.get(2).map(integer_argument).transpose()?.unwrap_or(1);
    let hour = args.get(3).map(integer_argument).transpose()?.unwrap_or(0);
    let minute = args.get(4).map(integer_argument).transpose()?.unwrap_or(0);
    let second = args.get(5).map(integer_argument).transpose()?.unwrap_or(0);
    let millis = args.get(6).map(integer_argument).transpose()?.unwrap_or(0);
    if !(-100_000..=100_000).contains(&normalized_year) {
        return Err(EvalError::new(
            "range_error",
            "Date.UTC year is outside the supported range",
            None,
        ));
    }
    let day_offset = day.checked_sub(1).ok_or_else(number_overflow)?;
    let hour_millis = hour.checked_mul(3_600_000).ok_or_else(number_overflow)?;
    let minute_millis = minute.checked_mul(60_000).ok_or_else(number_overflow)?;
    let second_millis = second.checked_mul(1_000).ok_or_else(number_overflow)?;
    let days = days_from_civil(normalized_year, normalized_month, 1)
        .checked_add(day_offset)
        .ok_or_else(number_overflow)?;
    days.checked_mul(86_400_000)
        .and_then(|value| value.checked_add(hour_millis))
        .and_then(|value| value.checked_add(minute_millis))
        .and_then(|value| value.checked_add(second_millis))
        .and_then(|value| value.checked_add(millis))
        .ok_or_else(number_overflow)
}

fn parse_two_digits(value: &str, start: usize) -> Result<u32, EvalError> {
    value
        .get(start..start + 2)
        .ok_or_else(invalid_datetime)?
        .parse()
        .map_err(|_| invalid_datetime())
}

fn validate_date(year: i64, month: u32, day: u32) -> Result<(), EvalError> {
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => return Err(invalid_date()),
    };
    if day == 0 || day > max_day {
        return Err(invalid_date());
    }
    Ok(())
}

fn is_leap_year(year: i64) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

// Howard Hinnant's civil-date algorithm, returning days relative to 1970-01-01.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn invalid_date() -> EvalError {
    EvalError::new("invalid_date", "invalid ISO date", None)
}

fn invalid_datetime() -> EvalError {
    EvalError::new("invalid_datetime", "invalid ISO 8601 datetime", None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields(value: JsonValue) -> JsonMap<String, JsonValue> {
        value.as_object().cloned().expect("test fields object")
    }

    fn evaluate(
        formula: &str,
        result_type: FormulaResultType,
        input: JsonValue,
    ) -> FormulaEvaluation {
        FormulaEngine::default()
            .compile([FormulaDefinition::new("result", formula, result_type)])
            .evaluate(&fields(input))
    }

    #[test]
    fn decimal_arithmetic_is_exact() {
        let result = evaluate("0.1 + 0.2 === 0.3", FormulaResultType::Boolean, json!({}));
        assert_eq!(
            result.values.get("result"),
            Some(&FormulaValue::Boolean(true))
        );
        assert!(result.errors.is_empty());
    }

    #[test]
    fn oversized_numeric_inputs_report_overflow_on_the_formula() {
        let input: JsonValue = serde_json::from_str(r#"{"amount":1e100}"#).unwrap();
        let result = evaluate("amount + 1", FormulaResultType::Number, input);
        assert_eq!(result.errors["result"].code, "number_overflow");
        assert!(result.errors["result"]
            .message
            .contains("input field `amount`"));
    }

    #[test]
    fn exposes_identifier_and_fields_object_inputs() {
        let result = evaluate(
            "price * quantity + fields[\"Shipping Cost\"]",
            FormulaResultType::Number,
            json!({"price": 12.50, "quantity": 2, "Shipping Cost": 4.25}),
        );
        assert_eq!(
            result.values.get("result"),
            Some(&FormulaValue::Number(Decimal::new(2925, 2)))
        );
    }

    #[test]
    fn evaluates_formula_dependencies_topologically() {
        let program = FormulaEngine::default().compile([
            FormulaDefinition::new("grandTotal", "subtotal + tax", FormulaResultType::Number),
            FormulaDefinition::new("tax", "subtotal * 0.2", FormulaResultType::Number),
            FormulaDefinition::new("subtotal", "price * quantity", FormulaResultType::Number),
        ]);
        let result = program.evaluate(&fields(json!({"price": 10, "quantity": 3})));
        assert_eq!(
            result.values.get("grandTotal"),
            Some(&FormulaValue::Number(Decimal::from(36)))
        );
        assert!(result.errors.is_empty());
    }

    #[test]
    fn builtin_named_formula_fields_remain_direct_dependencies() {
        let program = FormulaEngine::default().compile([
            FormulaDefinition::new("answer", "JSON + 1", FormulaResultType::Number),
            FormulaDefinition::new("JSON", "2", FormulaResultType::Number),
            FormulaDefinition::new("rounded", "Math.round(1.4)", FormulaResultType::Number),
        ]);
        let result = program.evaluate(&JsonMap::new());
        assert_eq!(
            result.values.get("answer"),
            Some(&FormulaValue::Number(Decimal::from(3)))
        );
        assert_eq!(
            result.values.get("rounded"),
            Some(&FormulaValue::Number(Decimal::ONE))
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn reports_cycles_for_every_participant() {
        let program = FormulaEngine::default().compile([
            FormulaDefinition::new("a", "b + 1", FormulaResultType::Number),
            FormulaDefinition::new("b", "a + 1", FormulaResultType::Number),
        ]);
        let result = program.evaluate(&JsonMap::new());
        assert_eq!(result.errors["a"].code, "dependency_cycle");
        assert_eq!(result.errors["b"].code, "dependency_cycle");
        assert!(result.values.is_empty());
    }

    #[test]
    fn dependent_of_cycle_is_reported_as_dependency_failure() {
        let program = FormulaEngine::default().compile([
            FormulaDefinition::new("a", "b + 1", FormulaResultType::Number),
            FormulaDefinition::new("b", "a + 1", FormulaResultType::Number),
            FormulaDefinition::new("downstream", "a + 1", FormulaResultType::Number),
        ]);
        let result = program.evaluate(&JsonMap::new());
        assert_eq!(result.errors["a"].code, "dependency_cycle");
        assert_eq!(result.errors["b"].code, "dependency_cycle");
        assert_eq!(result.errors["downstream"].code, "dependency_failed");
    }

    #[test]
    fn failed_dependency_does_not_leave_stale_value() {
        let program = FormulaEngine::default().compile([
            FormulaDefinition::new("subtotal", "missing * 2", FormulaResultType::Number),
            FormulaDefinition::new("total", "subtotal + 1", FormulaResultType::Number),
        ]);
        let result = program.evaluate(&JsonMap::new());
        assert_eq!(result.errors["subtotal"].code, "unknown_identifier");
        assert_eq!(result.errors["total"].code, "dependency_failed");
        assert!(result.values.is_empty());
    }

    #[test]
    fn supports_expression_arrow_callbacks() {
        let result = evaluate(
            "amounts.filter(amount => amount >= 2).map(amount => amount * 2).reduce((sum, amount) => sum + amount, 0)",
            FormulaResultType::Number,
            json!({"amounts": [1, 2, 3]}),
        );
        assert_eq!(
            result.values.get("result"),
            Some(&FormulaValue::Number(Decimal::from(10)))
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn supports_optional_chaining_nullish_and_templates() {
        let result = evaluate(
            "`${customer?.name ?? \"Unknown\"}`.trim()",
            FormulaResultType::String,
            json!({"customer": null}),
        );
        assert_eq!(
            result.values.get("result"),
            Some(&FormulaValue::String("Unknown".to_owned()))
        );

        let nested = evaluate(
            "customer?.address.city ?? \"Unknown\"",
            FormulaResultType::String,
            json!({"customer": null}),
        );
        assert_eq!(
            nested.values.get("result"),
            Some(&FormulaValue::String("Unknown".to_owned()))
        );

        let skipped_arguments = evaluate(
            "customer?.trim(missing) ?? \"safe\"",
            FormulaResultType::String,
            json!({"customer": null}),
        );
        assert_eq!(
            skipped_arguments.values.get("result"),
            Some(&FormulaValue::String("safe".to_owned()))
        );
        assert!(skipped_arguments.errors.is_empty());
    }

    #[test]
    fn supports_safe_regex_methods() {
        let result = evaluate(
            "tags.filter(tag => /^priority:/i.test(tag)).join(\", \")",
            FormulaResultType::String,
            json!({"tags": ["Priority:high", "normal", "priority:low"]}),
        );
        assert_eq!(
            result.values.get("result"),
            Some(&FormulaValue::String(
                "Priority:high, priority:low".to_owned()
            ))
        );
    }

    #[test]
    fn rejects_regex_backreferences() {
        assert_eq!(
            FormulaEngine::default()
                .validate("/(a)\\1/.test(value)", FormulaResultType::Boolean)
                .expect_err("backreference must fail validation")
                .code,
            "invalid_regex"
        );
        let result = evaluate(
            "/(a)\\1/.test(value)",
            FormulaResultType::Boolean,
            json!({"value": "aa"}),
        );
        assert_eq!(result.errors["result"].code, "invalid_regex");
    }

    #[test]
    fn rejects_loose_equality_and_dangerous_properties_at_compile_time() {
        let engine = FormulaEngine::default();
        assert_eq!(
            engine
                .validate("price == 1", FormulaResultType::Boolean)
                .expect_err("loose equality must fail")
                .code,
            "loose_equality"
        );
        assert_eq!(
            engine
                .validate("fields.constructor", FormulaResultType::Json)
                .expect_err("constructor access must fail")
                .code,
            "forbidden_property"
        );
        assert_eq!(
            engine
                .validate("eval(\"1\")", FormulaResultType::Number)
                .expect_err("eval must fail")
                .code,
            "forbidden_identifier"
        );
    }

    #[test]
    fn rejects_non_allowlisted_calls_before_row_evaluation() {
        let engine = FormulaEngine::default();
        for expression in [
            "Date.now() + price",
            "Math.random() + price",
            "items.sort()",
            "fields[method]()",
            "fields[map]()",
        ] {
            assert_eq!(
                engine
                    .validate(expression, FormulaResultType::Number)
                    .expect_err(expression)
                    .code,
                "unsupported_call"
            );
        }
    }

    #[test]
    fn supports_checked_numeric_predicates() {
        let result = evaluate(
            "isFinite(amount) && isInteger(amount)",
            FormulaResultType::Boolean,
            json!({"amount": 12}),
        );
        assert_eq!(
            result.values.get("result"),
            Some(&FormulaValue::Boolean(true))
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    #[test]
    fn validation_checks_constant_result_types_without_requiring_field_values() {
        let engine = FormulaEngine::default();
        assert_eq!(
            engine
                .validate("\"text\"", FormulaResultType::Number)
                .expect_err("constant type mismatch must fail")
                .code,
            "result_type"
        );
        assert!(engine
            .validate("price * quantity", FormulaResultType::Number)
            .is_ok());
    }

    #[test]
    fn block_callbacks_are_rejected() {
        let error = FormulaEngine::default()
            .validate(
                "items.map(item => { return item * 2; })",
                FormulaResultType::List,
            )
            .expect_err("block callback must fail");
        assert_eq!(error.code, "unsupported_callback");
    }

    #[test]
    fn materialized_formula_value_is_recomputed_instead_of_colliding() {
        let result = evaluate(
            "price * 2",
            FormulaResultType::Number,
            json!({"price": 3, "result": 99}),
        );
        assert!(result.errors.is_empty());
        assert_eq!(
            result.values["result"],
            FormulaValue::Number(Decimal::from(6))
        );
    }

    #[test]
    fn validates_declared_result_type() {
        let result = evaluate("\"12\"", FormulaResultType::Number, json!({}));
        assert_eq!(result.errors["result"].code, "result_type");
    }

    #[test]
    fn enforces_evaluation_budget_for_callbacks() {
        let limits = FormulaLimits {
            max_evaluation_steps: 8,
            ..FormulaLimits::default()
        };
        let program = FormulaEngine::new(limits).compile([FormulaDefinition::new(
            "result",
            "items.map(item => item + 1)",
            FormulaResultType::List,
        )]);
        let result = program.evaluate(&fields(json!({"items": [1, 2, 3, 4, 5]})));
        assert_eq!(result.errors["result"].code, "evaluation_limit");
    }

    #[test]
    fn rejects_oversized_padding_before_allocation() {
        let result = evaluate(
            "\"x\".padStart(9223372036854775807, \"x\")",
            FormulaResultType::String,
            json!({}),
        );
        assert_eq!(result.errors["result"].code, "output_limit");
    }

    #[test]
    fn deterministic_date_functions_use_utc() {
        let parsed = evaluate(
            "Date.parse(\"1970-01-02T00:00:00Z\")",
            FormulaResultType::Number,
            json!({}),
        );
        assert_eq!(
            parsed.values.get("result"),
            Some(&FormulaValue::Number(Decimal::from(86_400_000)))
        );

        let utc = evaluate("Date.UTC(1970, 0, 2)", FormulaResultType::Number, json!({}));
        assert_eq!(
            utc.values.get("result"),
            Some(&FormulaValue::Number(Decimal::from(86_400_000)))
        );

        let overflow = evaluate(
            "Date.UTC(1970, 0, 1, 9223372036854775807)",
            FormulaResultType::Number,
            json!({}),
        );
        assert_eq!(overflow.errors["result"].code, "number_overflow");

        let date_only = evaluate("\"2026-07-28\"", FormulaResultType::DateTime, json!({}));
        assert_eq!(date_only.errors["result"].code, "result_type");
    }

    #[test]
    fn common_string_and_array_methods_follow_javascript_boundaries() {
        let result = evaluate(
            r#"[
                "abcabc".includes("a", 1),
                "abc".startsWith("b", 1),
                "abc".endsWith("b", 2),
                "abc".charAt(-1),
                "abc".at(-1),
                "abc".split(undefined, 0)
            ]"#,
            FormulaResultType::List,
            json!({}),
        );
        assert_eq!(
            result.values.get("result"),
            Some(&FormulaValue::List(vec![
                FormulaValue::Boolean(true),
                FormulaValue::Boolean(true),
                FormulaValue::Boolean(true),
                FormulaValue::String(String::new()),
                FormulaValue::String("c".to_string()),
                FormulaValue::List(Vec::new()),
            ]))
        );
    }

    #[test]
    fn json_numbers_serialize_without_binary_float_roundtrip() {
        let value = FormulaValue::Number(
            Decimal::from_str("12345678901234567890.12345678").expect("decimal"),
        );
        let json = value.to_json().expect("serialize decimal");
        assert_eq!(json.to_string(), "12345678901234567890.12345678");
    }

    #[test]
    fn fingerprint_is_stable_and_sensitive_to_definition() {
        let engine = FormulaEngine::default();
        let first = engine.compile([FormulaDefinition::new(
            "total",
            "price * quantity",
            FormulaResultType::Number,
        )]);
        let same = engine.compile([FormulaDefinition::new(
            "total",
            "price * quantity",
            FormulaResultType::Number,
        )]);
        let changed = engine.compile([FormulaDefinition::new(
            "total",
            "price + quantity",
            FormulaResultType::Number,
        )]);
        assert_eq!(first.fingerprint(), same.fingerprint());
        assert_ne!(first.fingerprint(), changed.fingerprint());
    }
}
