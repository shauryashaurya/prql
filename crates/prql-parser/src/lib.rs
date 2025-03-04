mod expr;
mod interpolation;
pub mod lexer;
mod stmt;

use chumsky::{prelude::*, Stream};
use error::convert_lexer_error;
use error::convert_parser_error;
pub use error::Error;

use self::lexer::Token;

use prql_ast::stmt::*;
use prql_ast::Span;

mod error;
mod span;

use span::ParserSpan;

use common::PError;

/// Build PRQL AST from a PRQL query string.
pub fn parse_source(source: &str, source_id: u16) -> Result<Vec<Stmt>, Vec<Error>> {
    let mut errors = Vec::new();

    let (tokens, lex_errors) = ::chumsky::Parser::parse_recovery(&lexer::lexer(), source);

    errors.extend(
        lex_errors
            .into_iter()
            .map(|err| convert_lexer_error(source, err, source_id)),
    );

    let ast = if let Some(tokens) = tokens {
        let stream = prepare_stream(tokens, source, source_id);

        let (ast, parse_errors) = ::chumsky::Parser::parse_recovery(&stmt::source(), stream);

        errors.extend(parse_errors.into_iter().map(convert_parser_error));

        ast
    } else {
        None
    };

    if errors.is_empty() {
        Ok(ast.unwrap_or_default())
    } else {
        Err(errors)
    }
}

/// Helper that does not track source_ids
#[cfg(test)]
pub fn parse_single(source: &str) -> Result<Vec<Stmt>, Vec<Error>> {
    parse_source(source, 0)
}

fn prepare_stream(
    tokens: Vec<(Token, std::ops::Range<usize>)>,
    source: &str,
    source_id: u16,
) -> Stream<Token, ParserSpan, impl Iterator<Item = (Token, ParserSpan)> + Sized> {
    let tokens = tokens
        .into_iter()
        .map(move |(t, s)| (t, ParserSpan::new(source_id, s)));
    let len = source.chars().count();
    let eoi = ParserSpan(Span {
        start: len,
        end: len + 1,
        source_id,
    });
    Stream::from_iter(eoi, tokens)
}

mod common {
    use chumsky::prelude::*;

    use super::{lexer::Token, span::ParserSpan};
    use prql_ast::expr::*;
    use prql_ast::stmt::*;

    pub type PError = Simple<Token, ParserSpan>;

    pub fn ident_part() -> impl Parser<Token, String, Error = PError> {
        select! { Token::Ident(ident) => ident }.map_err(|e: PError| {
            Simple::expected_input_found(
                e.span(),
                [Some(Token::Ident("".to_string()))],
                e.found().cloned(),
            )
        })
    }

    pub fn keyword(kw: &'static str) -> impl Parser<Token, (), Error = PError> + Clone {
        just(Token::Keyword(kw.to_string())).ignored()
    }

    pub fn new_line() -> impl Parser<Token, (), Error = PError> + Clone {
        just(Token::NewLine).ignored()
    }

    pub fn ctrl(char: char) -> impl Parser<Token, (), Error = PError> + Clone {
        just(Token::Control(char)).ignored()
    }

    pub fn into_stmt((annotations, kind): (Vec<Annotation>, StmtKind), span: ParserSpan) -> Stmt {
        Stmt {
            kind,
            span: Some(span.0),
            annotations,
        }
    }

    pub fn into_expr(kind: ExprKind, span: ParserSpan) -> Expr {
        Expr {
            span: Some(span.0),
            ..Expr::new(kind)
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use insta::assert_yaml_snapshot;
    use prql_ast::expr::{Expr, FuncCall};

    fn parse_expr(source: &str) -> Result<Expr, Vec<Error>> {
        let tokens = Parser::parse(&lexer::lexer(), source).map_err(|errs| {
            errs.into_iter()
                .map(|err| convert_lexer_error(source, err, 0))
                .collect::<Vec<_>>()
        })?;

        let stream = prepare_stream(tokens, source, 0);
        Parser::parse(&expr::expr_call().then_ignore(end()), stream)
            .map_err(|errs| errs.into_iter().map(convert_parser_error).collect())
    }

    #[test]
    fn test_pipeline_parse_tree() {
        assert_yaml_snapshot!(parse_single(
            r#"
from employees
filter country == "USA"                      # Each line transforms the previous result.
derive {                                     # This adds columns / variables.
  gross_salary = salary + payroll_tax,
  gross_cost = gross_salary + benefits_cost  # Variables can use other variables.
}
filter gross_cost > 0
group {title, country} (                     # For each group use a nested pipeline
  aggregate {                                # Aggregate each group to a single row
    average salary,
    average gross_salary,
    sum salary,
    sum gross_salary,
    average gross_cost,
    sum_gross_cost = sum gross_cost,
    ct = count salary,
  }
)
sort sum_gross_cost
filter ct > 200
take 20
        "#
        )
        .unwrap());
    }

    #[test]
    fn test_take() {
        parse_single("take 10").unwrap();

        assert_yaml_snapshot!(parse_single(r#"take 10"#).unwrap(), @r###"
        ---
        - Main:
            FuncCall:
              name:
                Ident:
                  - take
              args:
                - Literal:
                    Integer: 10
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_single(r#"take ..10"#).unwrap(), @r###"
        ---
        - Main:
            FuncCall:
              name:
                Ident:
                  - take
              args:
                - Range:
                    start: ~
                    end:
                      Literal:
                        Integer: 10
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_single(r#"take 1..10"#).unwrap(), @r###"
        ---
        - Main:
            FuncCall:
              name:
                Ident:
                  - take
              args:
                - Range:
                    start:
                      Literal:
                        Integer: 1
                    end:
                      Literal:
                        Integer: 10
          annotations: []
        "###);
    }

    #[test]
    fn test_ranges() {
        assert_yaml_snapshot!(parse_expr(r#"3..5"#).unwrap(), @r###"
        ---
        Range:
          start:
            Literal:
              Integer: 3
          end:
            Literal:
              Integer: 5
        "###);

        assert_yaml_snapshot!(parse_expr(r#"-2..-5"#).unwrap(), @r###"
        ---
        Range:
          start:
            Unary:
              op: Neg
              expr:
                Literal:
                  Integer: 2
          end:
            Unary:
              op: Neg
              expr:
                Literal:
                  Integer: 5
        "###);

        assert_yaml_snapshot!(parse_expr(r#"(-2..(-5 | abs))"#).unwrap(), @r###"
        ---
        Range:
          start:
            Unary:
              op: Neg
              expr:
                Literal:
                  Integer: 2
          end:
            Pipeline:
              exprs:
                - Unary:
                    op: Neg
                    expr:
                      Literal:
                        Integer: 5
                - Ident:
                    - abs
        "###);

        assert_yaml_snapshot!(parse_expr(r#"(2 + 5)..'a'"#).unwrap(), @r###"
        ---
        Range:
          start:
            Binary:
              left:
                Literal:
                  Integer: 2
              op: Add
              right:
                Literal:
                  Integer: 5
          end:
            Literal:
              String: a
        "###);

        assert_yaml_snapshot!(parse_expr(r#"1.6..rel.col"#).unwrap(), @r###"
        ---
        Range:
          start:
            Literal:
              Float: 1.6
          end:
            Ident:
              - rel
              - col
        "###);

        assert_yaml_snapshot!(parse_expr(r#"6.."#).unwrap(), @r###"
        ---
        Range:
          start:
            Literal:
              Integer: 6
          end: ~
        "###);
        assert_yaml_snapshot!(parse_expr(r#"..7"#).unwrap(), @r###"
        ---
        Range:
          start: ~
          end:
            Literal:
              Integer: 7
        "###);

        assert_yaml_snapshot!(parse_expr(r#".."#).unwrap(), @r###"
        ---
        Range:
          start: ~
          end: ~
        "###);

        assert_yaml_snapshot!(parse_expr(r#"@2020-01-01..@2021-01-01"#).unwrap(), @r###"
        ---
        Range:
          start:
            Literal:
              Date: 2020-01-01
          end:
            Literal:
              Date: 2021-01-01
        "###);
    }

    #[test]
    fn test_basic_exprs() {
        assert_yaml_snapshot!(parse_expr(r#"country == "USA""#).unwrap(), @r###"
        ---
        Binary:
          left:
            Ident:
              - country
          op: Eq
          right:
            Literal:
              String: USA
        "###);
        assert_yaml_snapshot!(parse_expr("select {a, b, c}").unwrap(), @r###"
        ---
        FuncCall:
          name:
            Ident:
              - select
          args:
            - Tuple:
                - Ident:
                    - a
                - Ident:
                    - b
                - Ident:
                    - c
        "###);
        assert_yaml_snapshot!(parse_expr(
            "group {title, country} (
                aggregate {sum salary}
            )"
        ).unwrap(), @r###"
        ---
        FuncCall:
          name:
            Ident:
              - group
          args:
            - Tuple:
                - Ident:
                    - title
                - Ident:
                    - country
            - FuncCall:
                name:
                  Ident:
                    - aggregate
                args:
                  - Tuple:
                      - FuncCall:
                          name:
                            Ident:
                              - sum
                          args:
                            - Ident:
                                - salary
        "###);
        assert_yaml_snapshot!(parse_expr(
            r#"    filter country == "USA""#
        ).unwrap(), @r###"
        ---
        FuncCall:
          name:
            Ident:
              - filter
          args:
            - Binary:
                left:
                  Ident:
                    - country
                op: Eq
                right:
                  Literal:
                    String: USA
        "###);
        assert_yaml_snapshot!(parse_expr("{a, b, c,}").unwrap(), @r###"
        ---
        Tuple:
          - Ident:
              - a
          - Ident:
              - b
          - Ident:
              - c
        "###);
        assert_yaml_snapshot!(parse_expr(
            r#"{
  gross_salary = salary + payroll_tax,
  gross_cost   = gross_salary + benefits_cost
}"#
        ).unwrap(), @r###"
        ---
        Tuple:
          - Binary:
              left:
                Ident:
                  - salary
              op: Add
              right:
                Ident:
                  - payroll_tax
            alias: gross_salary
          - Binary:
              left:
                Ident:
                  - gross_salary
              op: Add
              right:
                Ident:
                  - benefits_cost
            alias: gross_cost
        "###);
        // Currently not putting comments in our parse tree, so this is blank.
        assert_yaml_snapshot!(parse_single(
            r#"# this is a comment
        select a"#
        ).unwrap(), @r###"
        ---
        - Main:
            FuncCall:
              name:
                Ident:
                  - select
              args:
                - Ident:
                    - a
          annotations: []
        "###);
        assert_yaml_snapshot!(parse_expr(
            "join side:left country (id==employee_id)"
        ).unwrap(), @r###"
        ---
        FuncCall:
          name:
            Ident:
              - join
          args:
            - Ident:
                - country
            - Binary:
                left:
                  Ident:
                    - id
                op: Eq
                right:
                  Ident:
                    - employee_id
          named_args:
            side:
              Ident:
                - left
        "###);
        assert_yaml_snapshot!(parse_expr("1  + 2").unwrap(), @r###"
        ---
        Binary:
          left:
            Literal:
              Integer: 1
          op: Add
          right:
            Literal:
              Integer: 2
        "###)
    }

    #[test]
    fn test_string() {
        let double_quoted_ast = parse_expr(r#"" U S A ""#).unwrap();
        assert_yaml_snapshot!(double_quoted_ast, @r###"
        ---
        Literal:
          String: " U S A "
        "###);

        let single_quoted_ast = parse_expr(r#"' U S A '"#).unwrap();
        assert_eq!(single_quoted_ast, double_quoted_ast);

        // Single quotes within double quotes should produce a string containing
        // the single quotes (and vice versa).
        assert_yaml_snapshot!(parse_expr(r#""' U S A '""#).unwrap(), @r###"
        ---
        Literal:
          String: "' U S A '"
        "###);
        assert_yaml_snapshot!(parse_expr(r#"'" U S A "'"#).unwrap(), @r###"
        ---
        Literal:
          String: "\" U S A \""
        "###);

        parse_expr(r#"" U S A"#).unwrap_err();
        parse_expr(r#"" U S A '"#).unwrap_err();

        assert_yaml_snapshot!(parse_expr(r#"" \nU S A ""#).unwrap(), @r###"
        ---
        Literal:
          String: " \nU S A "
        "###);

        assert_yaml_snapshot!(parse_expr(r#"r" \nU S A ""#).unwrap(), @r###"
        ---
        Literal:
          String: " \\nU S A "
        "###);

        let multi_double = parse_expr(
            r#""""
''
Canada
"

""""#,
        )
        .unwrap();
        assert_yaml_snapshot!(multi_double, @r###"
        ---
        Literal:
          String: "\n''\nCanada\n\"\n\n"
        "###);

        let multi_single = parse_expr(
            r#"'''
Canada
"
"""

'''"#,
        )
        .unwrap();
        assert_yaml_snapshot!(multi_single, @r###"
        ---
        Literal:
          String: "\nCanada\n\"\n\"\"\"\n\n"
        "###);

        assert_yaml_snapshot!(
          parse_expr("''").unwrap(),
          @r###"
        ---
        Literal:
          String: ""
        "###);
    }

    #[test]
    fn test_s_string() {
        assert_yaml_snapshot!(parse_expr(r#"s"SUM({col})""#).unwrap(), @r###"
        ---
        SString:
          - String: SUM(
          - Expr:
              expr:
                Ident:
                  - col
              format: ~
          - String: )
        "###);
        assert_yaml_snapshot!(parse_expr(r#"s"SUM({rel.`Col name`})""#).unwrap(), @r###"
        ---
        SString:
          - String: SUM(
          - Expr:
              expr:
                Ident:
                  - rel
                  - Col name
              format: ~
          - String: )
        "###)
    }

    #[test]
    fn test_s_string_braces() {
        assert_yaml_snapshot!(parse_expr(r#"s"{{?crystal_var}}""#).unwrap(), @r###"
        ---
        SString:
          - String: "{?crystal_var}"
        "###);
        assert_yaml_snapshot!(parse_expr(r#"s"foo{{bar""#).unwrap(), @r###"
        ---
        SString:
          - String: "foo{bar"
        "###);
        parse_expr(r#"s"foo{{bar}""#).unwrap_err();
    }

    #[test]
    fn test_tuple() {
        assert_yaml_snapshot!(parse_expr(r#"{1 + 1, 2}"#).unwrap(), @r###"
        ---
        Tuple:
          - Binary:
              left:
                Literal:
                  Integer: 1
              op: Add
              right:
                Literal:
                  Integer: 1
          - Literal:
              Integer: 2
        "###);
        assert_yaml_snapshot!(parse_expr(r#"{1 + (f 1), 2}"#).unwrap(), @r###"
        ---
        Tuple:
          - Binary:
              left:
                Literal:
                  Integer: 1
              op: Add
              right:
                FuncCall:
                  name:
                    Ident:
                      - f
                  args:
                    - Literal:
                        Integer: 1
          - Literal:
              Integer: 2
        "###);
        // Line breaks
        assert_yaml_snapshot!(parse_expr(
            r#"{1,

                2}"#
        ).unwrap(), @r###"
        ---
        Tuple:
          - Literal:
              Integer: 1
          - Literal:
              Integer: 2
        "###);
        // Function call in a tuple
        let ab = parse_expr(r#"{a b}"#).unwrap();
        let a_comma_b = parse_expr(r#"{a, b}"#).unwrap();
        assert_yaml_snapshot!(ab, @r###"
        ---
        Tuple:
          - FuncCall:
              name:
                Ident:
                  - a
              args:
                - Ident:
                    - b
        "###);
        assert_yaml_snapshot!(a_comma_b, @r###"
        ---
        Tuple:
          - Ident:
              - a
          - Ident:
              - b
        "###);
        assert_ne!(ab, a_comma_b);

        assert_yaml_snapshot!(parse_expr(r#"{amount, +amount, -amount}"#).unwrap(), @r###"
        ---
        Tuple:
          - Ident:
              - amount
          - Unary:
              op: Add
              expr:
                Ident:
                  - amount
          - Unary:
              op: Neg
              expr:
                Ident:
                  - amount
        "###);
        // Operators in tuple items
        assert_yaml_snapshot!(parse_expr(r#"{amount, +amount, -amount}"#).unwrap(), @r###"
        ---
        Tuple:
          - Ident:
              - amount
          - Unary:
              op: Add
              expr:
                Ident:
                  - amount
          - Unary:
              op: Neg
              expr:
                Ident:
                  - amount
        "###);
    }

    #[test]
    fn test_number() {
        assert_yaml_snapshot!(parse_expr(r#"23"#).unwrap(), @r###"
        ---
        Literal:
          Integer: 23
        "###);
        assert_yaml_snapshot!(parse_expr(r#"2_3_4.5_6"#).unwrap(), @r###"
        ---
        Literal:
          Float: 234.56
        "###);
        assert_yaml_snapshot!(parse_expr(r#"23.6"#).unwrap(), @r###"
        ---
        Literal:
          Float: 23.6
        "###);
        assert_yaml_snapshot!(parse_expr(r#"23.0"#).unwrap(), @r###"
        ---
        Literal:
          Float: 23
        "###);
        assert_yaml_snapshot!(parse_expr(r#"2 + 2"#).unwrap(), @r###"
        ---
        Binary:
          left:
            Literal:
              Integer: 2
          op: Add
          right:
            Literal:
              Integer: 2
        "###);

        // Underscores at the beginning are parsed as ident
        assert!(parse_expr("_2").unwrap().kind.into_ident().is_ok());
        assert!(parse_expr("_").unwrap().kind.into_ident().is_ok());

        // We don't allow trailing periods
        assert!(parse_expr(r#"add 1. 2"#).is_err());

        assert!(parse_expr("_2.3").is_err());

        assert_yaml_snapshot!(parse_expr(r#"2e3"#).unwrap(), @r###"
        ---
        Literal:
          Float: 2000
        "###);

        // expr_of_string("2_").unwrap_err(); // TODO
        // expr_of_string("2.3_").unwrap_err(); // TODO
    }

    #[test]
    fn test_filter() {
        assert_yaml_snapshot!(
            parse_single(r#"filter country == "USA""#).unwrap(), @r###"
        ---
        - Main:
            FuncCall:
              name:
                Ident:
                  - filter
              args:
                - Binary:
                    left:
                      Ident:
                        - country
                    op: Eq
                    right:
                      Literal:
                        String: USA
          annotations: []
        "###);

        assert_yaml_snapshot!(
            parse_single(r#"filter (upper country) == "USA""#).unwrap(), @r###"
        ---
        - Main:
            FuncCall:
              name:
                Ident:
                  - filter
              args:
                - Binary:
                    left:
                      FuncCall:
                        name:
                          Ident:
                            - upper
                        args:
                          - Ident:
                              - country
                    op: Eq
                    right:
                      Literal:
                        String: USA
          annotations: []
        "###
        );
    }

    #[test]
    fn test_aggregate() {
        let aggregate = parse_single(
            r"group {title} (
                aggregate {sum salary, count}
              )",
        )
        .unwrap();
        assert_yaml_snapshot!(
            aggregate, @r###"
        ---
        - Main:
            FuncCall:
              name:
                Ident:
                  - group
              args:
                - Tuple:
                    - Ident:
                        - title
                - FuncCall:
                    name:
                      Ident:
                        - aggregate
                    args:
                      - Tuple:
                          - FuncCall:
                              name:
                                Ident:
                                  - sum
                              args:
                                - Ident:
                                    - salary
                          - Ident:
                              - count
          annotations: []
        "###);
        let aggregate = parse_single(
            r"group {title} (
                aggregate {sum salary}
              )",
        )
        .unwrap();
        assert_yaml_snapshot!(
            aggregate, @r###"
        ---
        - Main:
            FuncCall:
              name:
                Ident:
                  - group
              args:
                - Tuple:
                    - Ident:
                        - title
                - FuncCall:
                    name:
                      Ident:
                        - aggregate
                    args:
                      - Tuple:
                          - FuncCall:
                              name:
                                Ident:
                                  - sum
                              args:
                                - Ident:
                                    - salary
          annotations: []
        "###);
    }

    #[test]
    fn test_derive() {
        assert_yaml_snapshot!(
            parse_expr(r#"derive {x = 5, y = (-x)}"#).unwrap()
        , @r###"
        ---
        FuncCall:
          name:
            Ident:
              - derive
          args:
            - Tuple:
                - Literal:
                    Integer: 5
                  alias: x
                - Unary:
                    op: Neg
                    expr:
                      Ident:
                        - x
                  alias: y
        "###);
    }

    #[test]
    fn test_select() {
        assert_yaml_snapshot!(
            parse_expr(r#"select x"#).unwrap()
        , @r###"
        ---
        FuncCall:
          name:
            Ident:
              - select
          args:
            - Ident:
                - x
        "###);

        assert_yaml_snapshot!(
            parse_expr(r#"select !{x}"#).unwrap()
        , @r###"
        ---
        FuncCall:
          name:
            Ident:
              - select
          args:
            - Unary:
                op: Not
                expr:
                  Tuple:
                    - Ident:
                        - x
        "###);

        assert_yaml_snapshot!(
            parse_expr(r#"select {x, y}"#).unwrap()
        , @r###"
        ---
        FuncCall:
          name:
            Ident:
              - select
          args:
            - Tuple:
                - Ident:
                    - x
                - Ident:
                    - y
        "###);
    }

    #[test]
    fn test_expr() {
        assert_yaml_snapshot!(
            parse_expr(r#"country == "USA""#).unwrap()
        , @r###"
        ---
        Binary:
          left:
            Ident:
              - country
          op: Eq
          right:
            Literal:
              String: USA
        "###);
        assert_yaml_snapshot!(parse_expr(
                r#"{
  gross_salary = salary + payroll_tax,
  gross_cost   = gross_salary + benefits_cost,
}"#).unwrap(), @r###"
        ---
        Tuple:
          - Binary:
              left:
                Ident:
                  - salary
              op: Add
              right:
                Ident:
                  - payroll_tax
            alias: gross_salary
          - Binary:
              left:
                Ident:
                  - gross_salary
              op: Add
              right:
                Ident:
                  - benefits_cost
            alias: gross_cost
        "###);
        assert_yaml_snapshot!(
            parse_expr(
                "(salary + payroll_tax) * (1 + tax_rate)"
            ).unwrap(),
            @r###"
        ---
        Binary:
          left:
            Binary:
              left:
                Ident:
                  - salary
              op: Add
              right:
                Ident:
                  - payroll_tax
          op: Mul
          right:
            Binary:
              left:
                Literal:
                  Integer: 1
              op: Add
              right:
                Ident:
                  - tax_rate
        "###);
    }

    #[test]
    fn test_regex() {
        assert_yaml_snapshot!(
            parse_expr(
                "'oba' ~= 'foobar'"
            ).unwrap(),
            @r###"
        ---
        Binary:
          left:
            Literal:
              String: oba
          op: RegexSearch
          right:
            Literal:
              String: foobar
        "###);
    }

    #[test]
    fn test_function() {
        assert_yaml_snapshot!(parse_single("let plus_one = x ->  x + 1\n").unwrap(), @r###"
        ---
        - VarDef:
            name: plus_one
            value:
              Func:
                return_ty: ~
                body:
                  Binary:
                    left:
                      Ident:
                        - x
                    op: Add
                    right:
                      Literal:
                        Integer: 1
                params:
                  - name: x
                    default_value: ~
                named_params: []
            ty_expr: ~
            kind: Let
          annotations: []
        "###);
        assert_yaml_snapshot!(parse_single("let identity = x ->  x\n").unwrap()
        , @r###"
        ---
        - VarDef:
            name: identity
            value:
              Func:
                return_ty: ~
                body:
                  Ident:
                    - x
                params:
                  - name: x
                    default_value: ~
                named_params: []
            ty_expr: ~
            kind: Let
          annotations: []
        "###);
        assert_yaml_snapshot!(parse_single("let plus_one = x ->  (x + 1)\n").unwrap()
        , @r###"
        ---
        - VarDef:
            name: plus_one
            value:
              Func:
                return_ty: ~
                body:
                  Binary:
                    left:
                      Ident:
                        - x
                    op: Add
                    right:
                      Literal:
                        Integer: 1
                params:
                  - name: x
                    default_value: ~
                named_params: []
            ty_expr: ~
            kind: Let
          annotations: []
        "###);
        assert_yaml_snapshot!(parse_single("let plus_one = x ->  x + 1\n").unwrap()
        , @r###"
        ---
        - VarDef:
            name: plus_one
            value:
              Func:
                return_ty: ~
                body:
                  Binary:
                    left:
                      Ident:
                        - x
                    op: Add
                    right:
                      Literal:
                        Integer: 1
                params:
                  - name: x
                    default_value: ~
                named_params: []
            ty_expr: ~
            kind: Let
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_single("let foo = x -> some_func (foo bar + 1) (plax) - baz\n").unwrap()
        , @r###"
        ---
        - VarDef:
            name: foo
            value:
              Func:
                return_ty: ~
                body:
                  FuncCall:
                    name:
                      Ident:
                        - some_func
                    args:
                      - FuncCall:
                          name:
                            Ident:
                              - foo
                          args:
                            - Binary:
                                left:
                                  Ident:
                                    - bar
                                op: Add
                                right:
                                  Literal:
                                    Integer: 1
                      - Binary:
                          left:
                            Ident:
                              - plax
                          op: Sub
                          right:
                            Ident:
                              - baz
                params:
                  - name: x
                    default_value: ~
                named_params: []
            ty_expr: ~
            kind: Let
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_single("func return_constant ->  42\n").unwrap(), @r###"
        ---
        - Main:
            Func:
              return_ty: ~
              body:
                Literal:
                  Integer: 42
              params:
                - name: return_constant
                  default_value: ~
              named_params: []
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_single(r#"let count = X -> s"SUM({X})"
        "#).unwrap(), @r###"
        ---
        - VarDef:
            name: count
            value:
              Func:
                return_ty: ~
                body:
                  SString:
                    - String: SUM(
                    - Expr:
                        expr:
                          Ident:
                            - X
                        format: ~
                    - String: )
                params:
                  - name: X
                    default_value: ~
                named_params: []
            ty_expr: ~
            kind: Let
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_single(
            r#"
            let lag_day = x ->  (
                window x
                by sec_id
                sort date
                lag 1
            )
        "#
        )
        .unwrap(), @r###"
        ---
        - VarDef:
            name: lag_day
            value:
              Func:
                return_ty: ~
                body:
                  Pipeline:
                    exprs:
                      - FuncCall:
                          name:
                            Ident:
                              - window
                          args:
                            - Ident:
                                - x
                      - FuncCall:
                          name:
                            Ident:
                              - by
                          args:
                            - Ident:
                                - sec_id
                      - FuncCall:
                          name:
                            Ident:
                              - sort
                          args:
                            - Ident:
                                - date
                      - FuncCall:
                          name:
                            Ident:
                              - lag
                          args:
                            - Literal:
                                Integer: 1
                params:
                  - name: x
                    default_value: ~
                named_params: []
            ty_expr: ~
            kind: Let
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_single("let add = x to:a ->  x + to\n").unwrap(), @r###"
        ---
        - VarDef:
            name: add
            value:
              Func:
                return_ty: ~
                body:
                  Binary:
                    left:
                      Ident:
                        - x
                    op: Add
                    right:
                      Ident:
                        - to
                params:
                  - name: x
                    default_value: ~
                named_params:
                  - name: to
                    default_value:
                      Ident:
                        - a
            ty_expr: ~
            kind: Let
          annotations: []
        "###);
    }

    #[test]
    fn test_func_call() {
        // Function without argument
        let ast = parse_expr(r#"count"#).unwrap();
        let ident = ast.kind.into_ident().unwrap();
        assert_yaml_snapshot!(
            ident, @r###"
        ---
        - count
        "###);

        let ast = parse_expr(r#"s 'foo'"#).unwrap();
        assert_yaml_snapshot!(
            ast, @r###"
        ---
        FuncCall:
          name:
            Ident:
              - s
          args:
            - Literal:
                String: foo
        "###);

        // A non-friendly option for #154
        let ast = parse_expr(r#"count s'*'"#).unwrap();
        let func_call: FuncCall = ast.kind.into_func_call().unwrap();
        assert_yaml_snapshot!(
            func_call, @r###"
        ---
        name:
          Ident:
            - count
        args:
          - SString:
              - String: "*"
        "###);

        parse_expr("plus_one x:0 x:0 ").unwrap_err();

        let ast = parse_expr(r#"add bar to=3"#).unwrap();
        assert_yaml_snapshot!(
            ast, @r###"
        ---
        FuncCall:
          name:
            Ident:
              - add
          args:
            - Ident:
                - bar
            - Literal:
                Integer: 3
              alias: to
        "###);
    }

    #[test]
    fn test_op_precedence() {
        assert_yaml_snapshot!(parse_expr(r#"1 + 2 - 3 - 4"#).unwrap(), @r###"
        ---
        Binary:
          left:
            Binary:
              left:
                Binary:
                  left:
                    Literal:
                      Integer: 1
                  op: Add
                  right:
                    Literal:
                      Integer: 2
              op: Sub
              right:
                Literal:
                  Integer: 3
          op: Sub
          right:
            Literal:
              Integer: 4
        "###);

        assert_yaml_snapshot!(parse_expr(r#"1 / 2 - 3 * 4 + 1"#).unwrap(), @r###"
        ---
        Binary:
          left:
            Binary:
              left:
                Binary:
                  left:
                    Literal:
                      Integer: 1
                  op: DivFloat
                  right:
                    Literal:
                      Integer: 2
              op: Sub
              right:
                Binary:
                  left:
                    Literal:
                      Integer: 3
                  op: Mul
                  right:
                    Literal:
                      Integer: 4
          op: Add
          right:
            Literal:
              Integer: 1
        "###);

        assert_yaml_snapshot!(parse_expr(r#"a && b || !c && d"#).unwrap(), @r###"
        ---
        Binary:
          left:
            Binary:
              left:
                Ident:
                  - a
              op: And
              right:
                Ident:
                  - b
          op: Or
          right:
            Binary:
              left:
                Unary:
                  op: Not
                  expr:
                    Ident:
                      - c
              op: And
              right:
                Ident:
                  - d
        "###);

        assert_yaml_snapshot!(parse_expr(r#"a && b + c || (d e) && f"#).unwrap(), @r###"
        ---
        Binary:
          left:
            Binary:
              left:
                Ident:
                  - a
              op: And
              right:
                Binary:
                  left:
                    Ident:
                      - b
                  op: Add
                  right:
                    Ident:
                      - c
          op: Or
          right:
            Binary:
              left:
                FuncCall:
                  name:
                    Ident:
                      - d
                  args:
                    - Ident:
                        - e
              op: And
              right:
                Ident:
                  - f
        "###);
    }

    #[test]
    fn test_var_def() {
        assert_yaml_snapshot!(parse_single(
            "let newest_employees = (from employees)"
        ).unwrap(), @r###"
        ---
        - VarDef:
            name: newest_employees
            value:
              FuncCall:
                name:
                  Ident:
                    - from
                args:
                  - Ident:
                      - employees
            ty_expr: ~
            kind: Let
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_single(
            r#"
        let newest_employees = (
          from employees
          group country (
            aggregate {
                average_country_salary = average salary
            }
          )
          sort tenure
          take 50
        )"#.trim()).unwrap(),
         @r###"
        ---
        - VarDef:
            name: newest_employees
            value:
              Pipeline:
                exprs:
                  - FuncCall:
                      name:
                        Ident:
                          - from
                      args:
                        - Ident:
                            - employees
                  - FuncCall:
                      name:
                        Ident:
                          - group
                      args:
                        - Ident:
                            - country
                        - FuncCall:
                            name:
                              Ident:
                                - aggregate
                            args:
                              - Tuple:
                                  - FuncCall:
                                      name:
                                        Ident:
                                          - average
                                      args:
                                        - Ident:
                                            - salary
                                    alias: average_country_salary
                  - FuncCall:
                      name:
                        Ident:
                          - sort
                      args:
                        - Ident:
                            - tenure
                  - FuncCall:
                      name:
                        Ident:
                          - take
                      args:
                        - Literal:
                            Integer: 50
            ty_expr: ~
            kind: Let
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_single(r#"
            let e = s"SELECT * FROM employees"
            "#).unwrap(), @r###"
        ---
        - VarDef:
            name: e
            value:
              SString:
                - String: SELECT * FROM employees
            ty_expr: ~
            kind: Let
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_single(
          "let x = (

            from x_table

            select only_in_x = foo

          )

          from x"
        ).unwrap(), @r###"
        ---
        - VarDef:
            name: x
            value:
              Pipeline:
                exprs:
                  - FuncCall:
                      name:
                        Ident:
                          - from
                      args:
                        - Ident:
                            - x_table
                  - FuncCall:
                      name:
                        Ident:
                          - select
                      args:
                        - Ident:
                            - foo
                          alias: only_in_x
            ty_expr: ~
            kind: Let
          annotations: []
        - Main:
            FuncCall:
              name:
                Ident:
                  - from
              args:
                - Ident:
                    - x
          annotations: []
        "###);
    }

    #[test]
    fn test_inline_pipeline() {
        assert_yaml_snapshot!(parse_expr("(salary | percentile 50)").unwrap(), @r###"
        ---
        Pipeline:
          exprs:
            - Ident:
                - salary
            - FuncCall:
                name:
                  Ident:
                    - percentile
                args:
                  - Literal:
                      Integer: 50
        "###);
        assert_yaml_snapshot!(parse_single("let median = x -> (x | percentile 50)\n").unwrap(), @r###"
        ---
        - VarDef:
            name: median
            value:
              Func:
                return_ty: ~
                body:
                  Pipeline:
                    exprs:
                      - Ident:
                          - x
                      - FuncCall:
                          name:
                            Ident:
                              - percentile
                          args:
                            - Literal:
                                Integer: 50
                params:
                  - name: x
                    default_value: ~
                named_params: []
            ty_expr: ~
            kind: Let
          annotations: []
        "###);
    }

    #[test]
    fn test_sql_parameters() {
        assert_yaml_snapshot!(parse_single(r#"
        from mytable
        filter {
          first_name == $1,
          last_name == $2.name
        }
        "#).unwrap(), @r###"
        ---
        - Main:
            Pipeline:
              exprs:
                - FuncCall:
                    name:
                      Ident:
                        - from
                    args:
                      - Ident:
                          - mytable
                - FuncCall:
                    name:
                      Ident:
                        - filter
                    args:
                      - Tuple:
                          - Binary:
                              left:
                                Ident:
                                  - first_name
                              op: Eq
                              right:
                                Param: "1"
                          - Binary:
                              left:
                                Ident:
                                  - last_name
                              op: Eq
                              right:
                                Param: 2.name
          annotations: []
        "###);
    }

    #[test]
    fn test_tab_characters() {
        // #284
        parse_single(
            "from c_invoice
join doc:c_doctype (==c_invoice_id)
select [
\tinvoice_no,
\tdocstatus
]",
        )
        .unwrap();
    }

    #[test]
    fn test_backticks() {
        let prql = "
from `a/*.parquet`
aggregate {max c}
join `schema.table` (==id)
join `my-proj.dataset.table`
join `my-proj`.`dataset`.`table`
";

        assert_yaml_snapshot!(parse_single(prql).unwrap(), @r###"
        ---
        - Main:
            Pipeline:
              exprs:
                - FuncCall:
                    name:
                      Ident:
                        - from
                    args:
                      - Ident:
                          - a/*.parquet
                - FuncCall:
                    name:
                      Ident:
                        - aggregate
                    args:
                      - Tuple:
                          - FuncCall:
                              name:
                                Ident:
                                  - max
                              args:
                                - Ident:
                                    - c
                - FuncCall:
                    name:
                      Ident:
                        - join
                    args:
                      - Ident:
                          - schema.table
                      - Unary:
                          op: EqSelf
                          expr:
                            Ident:
                              - id
                - FuncCall:
                    name:
                      Ident:
                        - join
                    args:
                      - Ident:
                          - my-proj.dataset.table
                - FuncCall:
                    name:
                      Ident:
                        - join
                    args:
                      - Ident:
                          - my-proj
                          - dataset
                          - table
          annotations: []
        "###);
    }

    #[test]
    fn test_sort() {
        assert_yaml_snapshot!(parse_single("
        from invoices
        sort issued_at
        sort (-issued_at)
        sort {issued_at}
        sort {-issued_at}
        sort {issued_at, -amount, +num_of_articles}
        ").unwrap(), @r###"
        ---
        - Main:
            Pipeline:
              exprs:
                - FuncCall:
                    name:
                      Ident:
                        - from
                    args:
                      - Ident:
                          - invoices
                - FuncCall:
                    name:
                      Ident:
                        - sort
                    args:
                      - Ident:
                          - issued_at
                - FuncCall:
                    name:
                      Ident:
                        - sort
                    args:
                      - Unary:
                          op: Neg
                          expr:
                            Ident:
                              - issued_at
                - FuncCall:
                    name:
                      Ident:
                        - sort
                    args:
                      - Tuple:
                          - Ident:
                              - issued_at
                - FuncCall:
                    name:
                      Ident:
                        - sort
                    args:
                      - Tuple:
                          - Unary:
                              op: Neg
                              expr:
                                Ident:
                                  - issued_at
                - FuncCall:
                    name:
                      Ident:
                        - sort
                    args:
                      - Tuple:
                          - Ident:
                              - issued_at
                          - Unary:
                              op: Neg
                              expr:
                                Ident:
                                  - amount
                          - Unary:
                              op: Add
                              expr:
                                Ident:
                                  - num_of_articles
          annotations: []
        "###);
    }

    #[test]
    fn test_dates() {
        assert_yaml_snapshot!(parse_single("
        from employees
        derive {age_plus_two_years = (age + 2years)}
        ").unwrap(), @r###"
        ---
        - Main:
            Pipeline:
              exprs:
                - FuncCall:
                    name:
                      Ident:
                        - from
                    args:
                      - Ident:
                          - employees
                - FuncCall:
                    name:
                      Ident:
                        - derive
                    args:
                      - Tuple:
                          - Binary:
                              left:
                                Ident:
                                  - age
                              op: Add
                              right:
                                Literal:
                                  ValueAndUnit:
                                    n: 2
                                    unit: years
                            alias: age_plus_two_years
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_expr("@2011-02-01").unwrap(), @r###"
        ---
        Literal:
          Date: 2011-02-01
        "###);
        assert_yaml_snapshot!(parse_expr("@2011-02-01T10:00").unwrap(), @r###"
        ---
        Literal:
          Timestamp: "2011-02-01T10:00"
        "###);
        assert_yaml_snapshot!(parse_expr("@14:00").unwrap(), @r###"
        ---
        Literal:
          Time: "14:00"
        "###);
        // assert_yaml_snapshot!(parse_expr("@2011-02-01T10:00<datetime>").unwrap(), @"");

        parse_expr("@2020-01-0").unwrap_err();

        parse_expr("@2020-01-011").unwrap_err();

        parse_expr("@2020-01-01T111").unwrap_err();
    }

    #[test]
    fn test_multiline_string() {
        assert_yaml_snapshot!(parse_single(r##"
        derive x = r#"r-string test"#
        "##).unwrap(), @r###"
        ---
        - Main:
            FuncCall:
              name:
                Ident:
                  - derive
              args:
                - Ident:
                    - r
                  alias: x
          annotations: []
        "### )
    }

    #[test]
    fn test_coalesce() {
        assert_yaml_snapshot!(parse_single(r###"
        from employees
        derive amount = amount ?? 0
        "###).unwrap(), @r###"
        ---
        - Main:
            Pipeline:
              exprs:
                - FuncCall:
                    name:
                      Ident:
                        - from
                    args:
                      - Ident:
                          - employees
                - FuncCall:
                    name:
                      Ident:
                        - derive
                    args:
                      - Binary:
                          left:
                            Ident:
                              - amount
                          op: Coalesce
                          right:
                            Literal:
                              Integer: 0
                        alias: amount
          annotations: []
        "### )
    }

    #[test]
    fn test_literal() {
        assert_yaml_snapshot!(parse_single(r###"
        derive x = true
        "###).unwrap(), @r###"
        ---
        - Main:
            FuncCall:
              name:
                Ident:
                  - derive
              args:
                - Literal:
                    Boolean: true
                  alias: x
          annotations: []
        "###)
    }

    #[test]
    fn test_allowed_idents() {
        assert_yaml_snapshot!(parse_single(r###"
        from employees
        join _salary (==employee_id) # table with leading underscore
        filter first_name == $1
        select {_employees._underscored_column}
        "###).unwrap(), @r###"
        ---
        - Main:
            Pipeline:
              exprs:
                - FuncCall:
                    name:
                      Ident:
                        - from
                    args:
                      - Ident:
                          - employees
                - FuncCall:
                    name:
                      Ident:
                        - join
                    args:
                      - Ident:
                          - _salary
                      - Unary:
                          op: EqSelf
                          expr:
                            Ident:
                              - employee_id
                - FuncCall:
                    name:
                      Ident:
                        - filter
                    args:
                      - Binary:
                          left:
                            Ident:
                              - first_name
                          op: Eq
                          right:
                            Param: "1"
                - FuncCall:
                    name:
                      Ident:
                        - select
                    args:
                      - Tuple:
                          - Ident:
                              - _employees
                              - _underscored_column
          annotations: []
        "###)
    }

    #[test]
    fn test_gt_lt_gte_lte() {
        assert_yaml_snapshot!(parse_single(r###"
        from people
        filter age >= 100
        filter num_grandchildren <= 10
        filter salary > 0
        filter num_eyes < 2
        "###).unwrap(), @r###"
        ---
        - Main:
            Pipeline:
              exprs:
                - FuncCall:
                    name:
                      Ident:
                        - from
                    args:
                      - Ident:
                          - people
                - FuncCall:
                    name:
                      Ident:
                        - filter
                    args:
                      - Binary:
                          left:
                            Ident:
                              - age
                          op: Gte
                          right:
                            Literal:
                              Integer: 100
                - FuncCall:
                    name:
                      Ident:
                        - filter
                    args:
                      - Binary:
                          left:
                            Ident:
                              - num_grandchildren
                          op: Lte
                          right:
                            Literal:
                              Integer: 10
                - FuncCall:
                    name:
                      Ident:
                        - filter
                    args:
                      - Binary:
                          left:
                            Ident:
                              - salary
                          op: Gt
                          right:
                            Literal:
                              Integer: 0
                - FuncCall:
                    name:
                      Ident:
                        - filter
                    args:
                      - Binary:
                          left:
                            Ident:
                              - num_eyes
                          op: Lt
                          right:
                            Literal:
                              Integer: 2
          annotations: []
        "###)
    }

    #[test]
    fn test_assign() {
        assert_yaml_snapshot!(parse_single(r###"
from employees
join s=salaries (==id)
        "###).unwrap(), @r###"
        ---
        - Main:
            Pipeline:
              exprs:
                - FuncCall:
                    name:
                      Ident:
                        - from
                    args:
                      - Ident:
                          - employees
                - FuncCall:
                    name:
                      Ident:
                        - join
                    args:
                      - Ident:
                          - salaries
                        alias: s
                      - Unary:
                          op: EqSelf
                          expr:
                            Ident:
                              - id
          annotations: []
        "###);
    }

    #[test]
    fn test_ident_with_keywords() {
        assert_yaml_snapshot!(parse_expr(r"select {andrew, orion, lettuce, falsehood, null0}").unwrap(), @r###"
        ---
        FuncCall:
          name:
            Ident:
              - select
          args:
            - Tuple:
                - Ident:
                    - andrew
                - Ident:
                    - orion
                - Ident:
                    - lettuce
                - Ident:
                    - falsehood
                - Ident:
                    - null0
        "###);

        assert_yaml_snapshot!(parse_expr(r"{false}").unwrap(), @r###"
        ---
        Tuple:
          - Literal:
              Boolean: false
        "###);
    }

    #[test]
    fn test_case() {
        assert_yaml_snapshot!(parse_expr(r#"case [
            nickname != null => nickname,
            true => null
        ]"#).unwrap(), @r###"
        ---
        Case:
          - condition:
              Binary:
                left:
                  Ident:
                    - nickname
                op: Ne
                right:
                  Literal: "Null"
            value:
              Ident:
                - nickname
          - condition:
              Literal:
                Boolean: true
            value:
              Literal: "Null"
        "###);
    }

    #[test]
    fn test_params() {
        assert_yaml_snapshot!(parse_expr(r#"$2"#).unwrap(), @r###"
        ---
        Param: "2"
        "###);

        assert_yaml_snapshot!(parse_expr(r#"$2_any_text"#).unwrap(), @r###"
        ---
        Param: 2_any_text
        "###);
    }

    #[test]
    fn test_unicode() {
        let source = "from tète";
        assert_yaml_snapshot!(parse_single(source).unwrap(), @r###"
        ---
        - Main:
            FuncCall:
              name:
                Ident:
                  - from
              args:
                - Ident:
                    - tète
          annotations: []
        "###);
    }

    #[test]
    fn test_var_defs() {
        assert_yaml_snapshot!(parse_single(r#"
        let a = (
            x
        )
        "#).unwrap(), @r###"
        ---
        - VarDef:
            name: a
            value:
              Ident:
                - x
            ty_expr: ~
            kind: Let
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_single(r#"
        x
        into a
        "#).unwrap(), @r###"
        ---
        - VarDef:
            name: a
            value:
              Ident:
                - x
            ty_expr: ~
            kind: Into
          annotations: []
        "###);

        assert_yaml_snapshot!(parse_single(r#"
        x
        "#).unwrap(), @r###"
        ---
        - Main:
            Ident:
              - x
          annotations: []
        "###);
    }

    #[test]
    fn test_array() {
        assert_yaml_snapshot!(parse_single(r#"
        let a = [1, 2,]
        let a = [false, "hello"]
        "#).unwrap(), @r###"
        ---
        - VarDef:
            name: a
            value:
              Array:
                - Literal:
                    Integer: 1
                - Literal:
                    Integer: 2
            ty_expr: ~
            kind: Let
          annotations: []
        - VarDef:
            name: a
            value:
              Array:
                - Literal:
                    Boolean: false
                - Literal:
                    String: hello
            ty_expr: ~
            kind: Let
          annotations: []
        "###);
    }

    #[test]
    fn test_annotation() {
        assert_yaml_snapshot!(parse_single(r#"
        @{binding_strength=1}
        let add = a b -> a + b
        "#).unwrap(), @r###"
        ---
        - VarDef:
            name: add
            value:
              Func:
                return_ty: ~
                body:
                  Binary:
                    left:
                      Ident:
                        - a
                    op: Add
                    right:
                      Ident:
                        - b
                params:
                  - name: a
                    default_value: ~
                  - name: b
                    default_value: ~
                named_params: []
            ty_expr: ~
            kind: Let
          annotations:
            - expr:
                Tuple:
                  - Literal:
                      Integer: 1
                    alias: binding_strength
        "###);
        parse_single(
            r#"
        @{binding_strength=1} let add = a b -> a + b
        "#,
        )
        .unwrap();

        parse_single(
            r#"
        @{binding_strength=1}
        # comment
        let add = a b -> a + b
        "#,
        )
        .unwrap();

        parse_single(
            r#"
        @{binding_strength=1}


        let add = a b -> a + b
        "#,
        )
        .unwrap();
    }

    #[test]
    fn check_valid_version() {
        let stmt = format!(
            r#"
        prql version:"{}"
        "#,
            env!("CARGO_PKG_VERSION_MAJOR")
        );
        assert!(parse_single(&stmt).is_ok());

        let stmt = format!(
            r#"
            prql version:"{}.{}"
            "#,
            env!("CARGO_PKG_VERSION_MAJOR"),
            env!("CARGO_PKG_VERSION_MINOR")
        );
        assert!(parse_single(&stmt).is_ok());

        let stmt = format!(
            r#"
            prql version:"{}.{}.{}"
            "#,
            env!("CARGO_PKG_VERSION_MAJOR"),
            env!("CARGO_PKG_VERSION_MINOR"),
            env!("CARGO_PKG_VERSION_PATCH"),
        );
        assert!(parse_single(&stmt).is_ok());
    }

    #[test]
    fn check_invalid_version() {
        let stmt = format!(
            "prql version:{}\n",
            env!("CARGO_PKG_VERSION_MAJOR").parse::<usize>().unwrap() + 1
        );
        assert!(parse_single(&stmt).is_err());
    }

    #[test]
    fn test_target() {
        assert_yaml_snapshot!(parse_single(
            r#"
          prql target:sql.sqlite

          from film
          remove film2
        "#,
        )
        .unwrap(), @r###"
        ---
        - QueryDef:
            version: ~
            other:
              target: sql.sqlite
          annotations: []
        - Main:
            Pipeline:
              exprs:
                - FuncCall:
                    name:
                      Ident:
                        - from
                    args:
                      - Ident:
                          - film
                - FuncCall:
                    name:
                      Ident:
                        - remove
                    args:
                      - Ident:
                          - film2
          annotations: []
        "###);
    }
}
