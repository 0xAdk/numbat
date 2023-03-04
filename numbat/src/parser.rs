//! Numbat Parser
//!
//! Operator precedence, low to high
//! * postfix apply ("//")
//! * conversion ("->")
//! * addition ("+")
//! * subtraction ("-")
//! * multiplication ("*")
//! * division ("/")
//! * exponentiation ("^")
//! * unary minus ("-")
//!
//! Grammar:
//! ```txt
//! statement       →   expression | variable_decl | function_decl | dimension_decl | unit_decl | procedure_call
//!
//! variable_decl   →   …
//! function_decl   →   …
//! dimension_decl  →   …
//! unit_decl       →   …
//!
//! expression      →   postfix_apply
//! postfix_apply   →   conversion ( "//" identifier ) *
//! conversion      →   term ( "→" term ) *
//! term            →   factor ( ( "+" | "-") factor ) *
//! factor          →   unary ( ( "*" | "/") unary ) *
//! unary           →   "-" unary | ifactor
//! ifactor         →   power ( " " power ) *
//! power           →   unicode_power ( "^" power )
//! unicode_power   →   call ( "⁻" ? ("¹" | "²" | "³" | "⁴" | "⁵" ) ) ?
//! call            →   primary ( "(" arguments? ")" ) ?
//! arguments       →   expression ( "," expression ) *
//! primary         →   number | identifier | "(" expression ")"
//! ```

use crate::arithmetic::{Exponent, Rational};
use crate::ast::{BinaryOperator, DimensionExpression, Expression, ProcedureKind, Statement};
use crate::decorator::Decorator;
use crate::number::Number;
use crate::span::Span;
use crate::tokenizer::{Token, TokenKind, TokenizerError};

use num_traits::{FromPrimitive, Zero};
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum ParseErrorKind {
    #[error("Unexpected character '{0}'")]
    TokenizerUnexpectedCharacter(char),

    #[error("Unexpected character in negative exponent")]
    TokenizerUnexpectedCharacterInNegativeExponent(Option<char>),

    #[error("Expected one of: number, identifier, parenthesized expression")]
    ExpectedPrimary,

    #[error("Missing closing parenthesis ')'")]
    MissingClosingParen,

    #[error("Trailing characters: '{0}'")]
    TrailingCharacters(String),

    #[error("Expected identifier after 'let' keyword")]
    ExpectedIdentifierAfterLet,

    #[error("Expected '=' or ':' after identifier in 'let' assignment")]
    ExpectedEqualOrColonAfterLetIdentifier,

    #[error("Expected identifier after 'fn' keyword. Note that some reserved words can not be used as function names.")]
    ExpectedIdentifierAfterFn,

    #[error("Expected function name after '//' operator")]
    ExpectedIdentifierInPostfixApply,

    #[error("Expected dimension identifier, '1', or opening parenthesis")]
    ExpectedDimensionPrimary,

    #[error("Expected ',' or '>'")]
    ExpectedCommaOrRightAngleBracket,

    #[error("Expected identifier (type parameter name)")]
    ExpectedTypeParameterName,

    #[error("Expected opening parenthesis '(' in function definition")]
    ExpectedLeftParenInFunctionDefinition,

    #[error("Expected ',' or ')' in function parameter list")]
    ExpectedCommaOrRightParenInFunctionDefinition,

    #[error("Expected parameter name in function definition")]
    ExpectedParameterNameInFunctionDefinition,

    #[error("Expected identifier (dimension name)")]
    ExpectedIdentifierAfterDimension,

    #[error("Expected identifier (unit name)")]
    ExpectedIdentifierAfterUnit,

    #[error("Expected '=' or ':' after identifier in unit definition")]
    ExpectedColonOrEqualAfterUnitIdentifier,

    #[error("Only functions can be called")]
    CanOnlyCallIdentifier,

    #[error("Division by zero in dimension exponent")]
    DivisionByZeroInDimensionExponent,

    #[error("Expected opening parenthesis '(' after procedure name")]
    ExpectedLeftParenAfterProcedureName,

    #[error("Procedures can not be used inside an expression")]
    InlineProcedureUsage,
}

#[derive(Debug, Error)]
#[error("Parse error in {span}: {kind}")]
pub struct ParseError {
    kind: ParseErrorKind,
    pub span: Span,
}

impl ParseError {
    fn new(kind: ParseErrorKind, span: Span) -> Self {
        ParseError { kind, span }
    }
}

type Result<T> = std::result::Result<T, ParseError>;

pub struct Parser<'a> {
    tokens: &'a [Token],
    current: usize,
    decorator_stack: Vec<Decorator>,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(tokens: &'a [Token]) -> Self {
        Parser {
            tokens,
            current: 0,
            decorator_stack: vec![],
        }
    }

    fn skip_empty_lines(&mut self) {
        while self.match_exact(TokenKind::Newline).is_some() {}
    }

    fn parse(&mut self) -> Result<Vec<Statement>> {
        let mut statements = vec![];

        self.skip_empty_lines();

        while !self.is_at_end() {
            match self.statement() {
                Ok(statement) => statements.push(statement),
                Err(e) => {
                    return Err(e);
                }
            }

            match self.peek().kind {
                TokenKind::Newline => {
                    // Skip over empty lines
                    while self.match_exact(TokenKind::Newline).is_some() {}
                }
                TokenKind::Eof => {
                    break;
                }
                _ => {
                    return Err(ParseError {
                        kind: ParseErrorKind::TrailingCharacters(self.peek().lexeme.clone()),
                        span: self.peek().span.clone(),
                    });
                }
            }
        }

        Ok(statements)
    }

    fn list_of_identifiers(&mut self) -> Result<Vec<String>> {
        if self.match_exact(TokenKind::RightParen).is_some() {
            return Ok(vec![]);
        }

        let mut identifiers: Vec<String> = vec![self.identifier()?];
        while self.match_exact(TokenKind::Comma).is_some() {
            identifiers.push(self.identifier()?);
        }

        if self.match_exact(TokenKind::RightParen).is_none() {
            return Err(ParseError::new(
                ParseErrorKind::MissingClosingParen,
                self.next().span.clone(),
            ));
        }

        Ok(identifiers)
    }

    fn statement(&mut self) -> Result<Statement> {
        if !(self.peek().kind == TokenKind::At
            || self.peek().kind == TokenKind::Unit
            || self.decorator_stack.is_empty())
        {
            todo!("Parser error: @-decorators can only be used on unit expressions")
        }

        if self.match_exact(TokenKind::Let).is_some() {
            if let Some(identifier) = self.match_exact(TokenKind::Identifier) {
                let dexpr = if self.match_exact(TokenKind::Colon).is_some() {
                    Some(self.dimension_expression()?)
                } else {
                    None
                };

                if self.match_exact(TokenKind::Equal).is_none() {
                    Err(ParseError {
                        kind: ParseErrorKind::ExpectedEqualOrColonAfterLetIdentifier,
                        span: self.peek().span.clone(),
                    })
                } else {
                    let expr = self.expression()?;
                    Ok(Statement::DeclareVariable(
                        identifier.lexeme.clone(),
                        expr,
                        dexpr,
                    ))
                }
            } else {
                Err(ParseError {
                    kind: ParseErrorKind::ExpectedIdentifierAfterLet,
                    span: self.peek().span.clone(),
                })
            }
        } else if self.match_exact(TokenKind::Fn).is_some() {
            if let Some(fn_name) = self.match_exact(TokenKind::Identifier) {
                let mut type_parameters = vec![];
                if self.match_exact(TokenKind::LeftAngleBracket).is_some() {
                    while self.match_exact(TokenKind::RightAngleBracket).is_none() {
                        if let Some(param_name) = self.match_exact(TokenKind::Identifier) {
                            type_parameters.push(param_name.lexeme.to_string());

                            if self.match_exact(TokenKind::Comma).is_none()
                                && self.peek().kind != TokenKind::RightAngleBracket
                            {
                                return Err(ParseError {
                                    kind: ParseErrorKind::ExpectedCommaOrRightAngleBracket,
                                    span: self.peek().span.clone(),
                                });
                            }
                        } else {
                            return Err(ParseError {
                                kind: ParseErrorKind::ExpectedTypeParameterName,
                                span: self.peek().span.clone(),
                            });
                        }
                    }
                }

                if self.match_exact(TokenKind::LeftParen).is_none() {
                    return Err(ParseError {
                        kind: ParseErrorKind::ExpectedLeftParenInFunctionDefinition,
                        span: self.peek().span.clone(),
                    });
                }

                let mut parameters = vec![];
                while self.match_exact(TokenKind::RightParen).is_none() {
                    if let Some(param_name) = self.match_exact(TokenKind::Identifier) {
                        let param_type_dexpr = if self.match_exact(TokenKind::Colon).is_some() {
                            Some(self.dimension_expression()?)
                        } else {
                            None
                        };

                        parameters.push((param_name.lexeme.to_string(), param_type_dexpr));

                        if self.match_exact(TokenKind::Comma).is_none()
                            && self.peek().kind != TokenKind::RightParen
                        {
                            return Err(ParseError {
                                kind: ParseErrorKind::ExpectedCommaOrRightParenInFunctionDefinition,
                                span: self.peek().span.clone(),
                            });
                        }
                    } else {
                        return Err(ParseError {
                            kind: ParseErrorKind::ExpectedParameterNameInFunctionDefinition,
                            span: self.peek().span.clone(),
                        });
                    }
                }

                let optional_return_type_dexpr = if self.match_exact(TokenKind::Arrow).is_some() {
                    // Parse return type
                    Some(self.dimension_expression()?)
                } else {
                    None
                };

                let body = if self.match_exact(TokenKind::Equal).is_none() {
                    None
                } else {
                    Some(self.expression()?)
                };
                Ok(Statement::DeclareFunction(
                    fn_name.lexeme.clone(),
                    type_parameters,
                    parameters,
                    body,
                    optional_return_type_dexpr,
                ))
            } else {
                Err(ParseError {
                    kind: ParseErrorKind::ExpectedIdentifierAfterFn,
                    span: self.peek().span.clone(),
                })
            }
        } else if self.match_exact(TokenKind::Dimension).is_some() {
            if let Some(identifier) = self.match_exact(TokenKind::Identifier) {
                if self.match_exact(TokenKind::Equal).is_some() {
                    let mut dexprs = vec![self.dimension_expression()?];

                    while self.match_exact(TokenKind::Equal).is_some() {
                        dexprs.push(self.dimension_expression()?);
                    }

                    Ok(Statement::DeclareDimension(
                        identifier.lexeme.clone(),
                        dexprs,
                    ))
                } else {
                    Ok(Statement::DeclareDimension(
                        identifier.lexeme.clone(),
                        vec![],
                    ))
                }
            } else {
                Err(ParseError {
                    kind: ParseErrorKind::ExpectedIdentifierAfterDimension,
                    span: self.peek().span.clone(),
                })
            }
        } else if self.match_exact(TokenKind::At).is_some() {
            if let Some(decorator) = self.match_exact(TokenKind::Identifier) {
                let decorator = if decorator.lexeme == "metric_prefixes" {
                    Decorator::MetricPrefixes
                } else if decorator.lexeme == "binary_prefixes" {
                    Decorator::BinaryPrefixes
                } else if decorator.lexeme == "aliases" {
                    if self.match_exact(TokenKind::LeftParen).is_some() {
                        let aliases = self.list_of_identifiers()?;
                        Decorator::Aliases(aliases)
                    } else {
                        todo!("Parse error: expected left paren after decorator")
                    }
                } else if decorator.lexeme == "aliases_short" {
                    if self.match_exact(TokenKind::LeftParen).is_some() {
                        let aliases_short = self.list_of_identifiers()?;
                        Decorator::AliasesShort(aliases_short)
                    } else {
                        todo!("Parse error: expected left paren after decorator")
                    }
                } else {
                    todo!("Parse error: unknown decorator")
                };

                self.decorator_stack.push(decorator); // TODO: make sure that there are no duplicate decorators

                // A decorator is not yet a full statement. Continue parsing:
                self.skip_empty_lines();
                self.statement()
            } else {
                todo!("Parse error: …")
            }
        } else if self.match_exact(TokenKind::Unit).is_some() {
            if let Some(identifier) = self.match_exact(TokenKind::Identifier) {
                let dexpr = if self.match_exact(TokenKind::Colon).is_some() {
                    Some(self.dimension_expression()?)
                } else {
                    None
                };

                let unit_name = identifier.lexeme.clone();

                let mut decorators = vec![];
                std::mem::swap(&mut decorators, &mut self.decorator_stack);

                if self.match_exact(TokenKind::Equal).is_some() {
                    let expr = self.expression()?;
                    Ok(Statement::DeclareDerivedUnit(
                        unit_name, expr, dexpr, decorators,
                    ))
                } else if let Some(dexpr) = dexpr {
                    Ok(Statement::DeclareBaseUnit(unit_name, dexpr, decorators))
                } else {
                    Err(ParseError {
                        kind: ParseErrorKind::ExpectedColonOrEqualAfterUnitIdentifier,
                        span: self.peek().span.clone(),
                    })
                }
            } else {
                Err(ParseError {
                    kind: ParseErrorKind::ExpectedIdentifierAfterUnit,
                    span: self.peek().span.clone(),
                })
            }
        } else if self
            .match_any(&[TokenKind::ProcedurePrint, TokenKind::ProcedureAssertEq])
            .is_some()
        {
            let procedure_kind = match self.last().unwrap().kind {
                TokenKind::ProcedurePrint => ProcedureKind::Print,
                TokenKind::ProcedureAssertEq => ProcedureKind::AssertEq,
                _ => unreachable!(),
            };

            if self.match_exact(TokenKind::LeftParen).is_none() {
                Err(ParseError {
                    kind: ParseErrorKind::ExpectedLeftParenAfterProcedureName,
                    span: self.peek().span.clone(),
                })
            } else {
                Ok(Statement::ProcedureCall(procedure_kind, self.arguments()?))
            }
        } else {
            Ok(Statement::Expression(self.expression()?))
        }
    }

    pub fn expression(&mut self) -> Result<Expression> {
        self.postfix_apply()
    }

    fn function_name_from_primary(&self, primary: Expression) -> Result<String> {
        if let Expression::Identifier(name) = primary {
            Ok(name)
        } else {
            Err(ParseError::new(
                ParseErrorKind::CanOnlyCallIdentifier,
                self.peek().span.clone(),
            ))
        }
    }

    fn identifier(&mut self) -> Result<String> {
        if let Some(identifier) = self.match_exact(TokenKind::Identifier) {
            Ok(identifier.lexeme.clone())
        } else {
            Err(ParseError::new(
                ParseErrorKind::ExpectedIdentifierInPostfixApply,
                self.peek().span.clone(),
            ))
        }
    }

    pub fn postfix_apply(&mut self) -> Result<Expression> {
        let mut expr = self.conversion()?;
        while self.match_exact(TokenKind::PostfixApply).is_some() {
            expr = Expression::FunctionCall(self.identifier()?, vec![expr]);
        }
        Ok(expr)
    }

    fn conversion(&mut self) -> Result<Expression> {
        let mut expr = self.term()?;
        while self.match_exact(TokenKind::Arrow).is_some() {
            let rhs = self.term()?;

            expr = Expression::BinaryOperator(
                BinaryOperator::ConvertTo,
                Box::new(expr),
                Box::new(rhs),
            );
        }
        Ok(expr)
    }

    fn term(&mut self) -> Result<Expression> {
        let mut expr = self.factor()?;
        while let Some(operator_token) = self.match_any(&[TokenKind::Plus, TokenKind::Minus]) {
            let operator = if operator_token.kind == TokenKind::Plus {
                BinaryOperator::Add
            } else {
                BinaryOperator::Sub
            };

            let rhs = self.factor()?;

            expr = Expression::BinaryOperator(operator, Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    fn factor(&mut self) -> Result<Expression> {
        let mut expr = self.unary()?;
        while let Some(operator_token) = self.match_any(&[TokenKind::Multiply, TokenKind::Divide]) {
            let operator = if operator_token.kind == TokenKind::Multiply {
                BinaryOperator::Mul
            } else {
                BinaryOperator::Div
            };

            let rhs = self.unary()?;

            expr = Expression::BinaryOperator(operator, Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    fn unary(&mut self) -> Result<Expression> {
        if self.match_exact(TokenKind::Minus).is_some() {
            let rhs = self.unary()?;

            Ok(Expression::Negate(Box::new(rhs)))
        } else {
            self.ifactor()
        }
    }

    fn ifactor(&mut self) -> Result<Expression> {
        let mut expr = self.power()?;

        while self.next_token_could_start_power_expression() {
            let rhs = self.power()?;
            expr = Expression::BinaryOperator(BinaryOperator::Mul, Box::new(expr), Box::new(rhs));
        }

        Ok(expr)
    }

    fn power(&mut self) -> Result<Expression> {
        let mut expr = self.unicode_power()?;
        if self.match_exact(TokenKind::Power).is_some() {
            let rhs = self.power()?;

            expr = Expression::BinaryOperator(BinaryOperator::Power, Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    fn unicode_power(&mut self) -> Result<Expression> {
        let mut expr = self.call()?;

        if let Some(exponent) = self.match_exact(TokenKind::UnicodeExponent) {
            let exp = match exponent.lexeme.as_str() {
                "⁻¹" => -1,
                "⁻²" => -2,
                "⁻³" => -3,
                "⁻⁴" => -4,
                "⁻⁵" => -5,
                "¹" => 1,
                "²" => 2,
                "³" => 3,
                "⁴" => 4,
                "⁵" => 5,
                _ => unreachable!(
                    "Tokenizer should not generate unicode exponent tokens for anything else"
                ),
            };

            expr = Expression::BinaryOperator(
                BinaryOperator::Power,
                Box::new(expr),
                Box::new(Expression::Scalar(Number::from_f64(exp as f64))),
            );
        }

        Ok(expr)
    }

    fn call(&mut self) -> Result<Expression> {
        let primary = self.primary()?;

        if self.match_exact(TokenKind::LeftParen).is_some() {
            let function_name = self.function_name_from_primary(primary)?;

            let args = self.arguments()?;
            return Ok(Expression::FunctionCall(function_name, args));
        }
        Ok(primary)
    }

    fn arguments(&mut self) -> Result<Vec<Expression>> {
        if self.match_exact(TokenKind::RightParen).is_some() {
            return Ok(vec![]);
        }

        let mut args: Vec<Expression> = vec![self.expression()?];
        while self.match_exact(TokenKind::Comma).is_some() {
            args.push(self.expression()?);
        }

        if self.match_exact(TokenKind::RightParen).is_none() {
            return Err(ParseError::new(
                ParseErrorKind::MissingClosingParen,
                self.next().span.clone(),
            ));
        }

        Ok(args)
    }

    fn primary(&mut self) -> Result<Expression> {
        // This function needs to be kept in sync with `next_token_could_start_primary` below.

        if let Some(num) = self.match_exact(TokenKind::Number) {
            Ok(Expression::Scalar(Number::from_f64(
                num.lexeme.parse::<f64>().unwrap(),
            )))
        } else if let Some(identifier) = self.match_exact(TokenKind::Identifier) {
            Ok(Expression::Identifier(identifier.lexeme.clone()))
        } else if self.match_exact(TokenKind::LeftParen).is_some() {
            let inner = self.expression()?;

            if self.match_exact(TokenKind::RightParen).is_none() {
                return Err(ParseError::new(
                    ParseErrorKind::MissingClosingParen,
                    self.next().span.clone(),
                ));
            }

            Ok(inner)
        } else if matches!(
            self.peek().kind,
            TokenKind::ProcedurePrint | TokenKind::ProcedureAssertEq
        ) {
            Err(ParseError::new(
                ParseErrorKind::InlineProcedureUsage,
                self.peek().span.clone(),
            ))
        } else {
            Err(ParseError::new(
                ParseErrorKind::ExpectedPrimary,
                self.peek().span.clone(),
            ))
        }
    }

    /// Returns true iff the upcoming token indicates the beginning of a 'power'
    /// expression (which needs to start with a 'primary' expression).
    fn next_token_could_start_power_expression(&self) -> bool {
        // This function needs to be kept in sync with `primary` above.

        matches!(
            self.peek().kind,
            TokenKind::Number | TokenKind::Identifier | TokenKind::LeftParen
        )
    }

    pub(crate) fn dimension_expression(&mut self) -> Result<DimensionExpression> {
        self.dimension_factor()
    }

    fn dimension_factor(&mut self) -> Result<DimensionExpression> {
        let mut expr = self.dimension_power()?;
        while let Some(operator_token) = self.match_any(&[TokenKind::Multiply, TokenKind::Divide]) {
            let rhs = self.dimension_power()?;

            expr = if operator_token.kind == TokenKind::Multiply {
                DimensionExpression::Multiply(Box::new(expr), Box::new(rhs))
            } else {
                DimensionExpression::Divide(Box::new(expr), Box::new(rhs))
            };
        }
        Ok(expr)
    }

    fn dimension_power(&mut self) -> Result<DimensionExpression> {
        let expr = self.dimension_primary()?;

        if self.match_exact(TokenKind::Power).is_some() {
            let exponent = self.dimension_exponent()?;

            Ok(DimensionExpression::Power(Box::new(expr), exponent))
        } else {
            Ok(expr)
        }
    }

    fn dimension_exponent(&mut self) -> Result<Exponent> {
        // TODO: potentially allow for ², ³, etc.

        if let Some(token) = self.match_exact(TokenKind::Number) {
            // TODO: only parse integers here
            Ok(Rational::from_f64(token.lexeme.parse::<f64>().unwrap()).unwrap())
        } else if self.match_exact(TokenKind::Minus).is_some() {
            let exponent = self.dimension_exponent()?;
            Ok(-exponent)
        } else if self.match_exact(TokenKind::LeftParen).is_some() {
            let exponent = self.dimension_exponent()?;
            if self.match_exact(TokenKind::RightParen).is_some() {
                Ok(exponent)
            } else if self.match_exact(TokenKind::Divide).is_some() {
                let rhs = self.dimension_exponent()?;
                if rhs == Rational::zero() {
                    Err(ParseError::new(
                        ParseErrorKind::DivisionByZeroInDimensionExponent,
                        self.last().unwrap().span.clone(),
                    ))
                } else if self.match_exact(TokenKind::RightParen).is_none() {
                    Err(ParseError::new(
                        ParseErrorKind::MissingClosingParen,
                        self.peek().span.clone(),
                    ))
                } else {
                    Ok(exponent / rhs)
                }
            } else {
                Err(ParseError::new(
                    ParseErrorKind::MissingClosingParen,
                    self.peek().span.clone(),
                ))
            }
        } else {
            todo!("parse error: expected integer number as dimension exponent")
        }
    }

    fn dimension_primary(&mut self) -> Result<DimensionExpression> {
        let e = Err(ParseError::new(
            ParseErrorKind::ExpectedDimensionPrimary,
            self.peek().span.clone(),
        ));
        if let Some(token) = self.match_exact(TokenKind::Identifier) {
            Ok(DimensionExpression::Dimension(token.lexeme.clone()))
        } else if let Some(number) = self.match_exact(TokenKind::Number) {
            if number.lexeme != "1" {
                e
            } else {
                Ok(DimensionExpression::Unity)
            }
        } else if self.match_exact(TokenKind::LeftParen).is_some() {
            let dexpr = self.dimension_expression()?;
            if self.match_exact(TokenKind::RightParen).is_none() {
                todo!("Parse error: expected ')'");
            }
            Ok(dexpr)
        } else {
            e
        }
    }

    fn match_exact(&mut self, token_kind: TokenKind) -> Option<&'a Token> {
        let token = self.peek();
        if token.kind == token_kind {
            self.advance();
            Some(token)
        } else {
            None
        }
    }

    fn match_any(&mut self, kinds: &[TokenKind]) -> Option<&'a Token> {
        for kind in kinds {
            if let result @ Some(..) = self.match_exact(*kind) {
                return result;
            }
        }
        None
    }

    fn advance(&mut self) {
        if !self.is_at_end() {
            self.current += 1;
        }
    }

    fn peek(&self) -> &'a Token {
        &self.tokens[self.current]
    }

    fn last(&self) -> Option<&'a Token> {
        self.tokens.get(self.current - 1)
    }

    fn next(&self) -> &'a Token {
        if self.is_at_end() {
            self.peek()
        } else {
            &self.tokens[self.current + 1]
        }
    }

    pub fn is_at_end(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }
}

pub fn parse(input: &str) -> Result<Vec<Statement>> {
    use crate::tokenizer::tokenize;

    let tokens = tokenize(input).map_err(|e| match e {
        TokenizerError::UnexpectedCharacter {
            character,
            ref span,
        } => ParseError::new(
            ParseErrorKind::TokenizerUnexpectedCharacter(character),
            span.clone(),
        ),
        TokenizerError::UnexpectedCharacterInNegativeExponent { character, span } => {
            ParseError::new(
                ParseErrorKind::TokenizerUnexpectedCharacterInNegativeExponent(character),
                span,
            )
        }
    })?;
    let mut parser = Parser::new(&tokens);
    parser.parse()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{binop, identifier, negate, scalar};

    fn parse_as(inputs: &[&str], statement_expected: Statement) {
        for input in inputs {
            let statements = parse(input).expect("parse error");

            assert!(statements.len() == 1);
            let statement = &statements[0];

            assert_eq!(*statement, statement_expected);
        }
    }

    fn parse_as_expression(inputs: &[&str], expr_expected: Expression) {
        parse_as(inputs, Statement::Expression(expr_expected));
    }

    fn should_fail(inputs: &[&str]) {
        for input in inputs {
            assert!(parse(input).is_err());
        }
    }

    fn should_fail_with(inputs: &[&str], error_kind: ParseErrorKind) {
        for input in inputs {
            match parse(input) {
                Err(e) => {
                    assert_eq!(e.kind, error_kind);
                }
                _ => {
                    assert!(false);
                }
            }
        }
    }

    #[test]
    fn invalid_input() {
        should_fail(&["+", "->", "§"]);

        should_fail_with(
            &["1)", "(1))"],
            ParseErrorKind::TrailingCharacters(")".into()),
        );
    }

    #[test]
    fn numbers_simple() {
        parse_as_expression(
            &[
                "1",
                "1.0",
                "  1   ",
                " 1.0000   ",
                "1.",
                // " +1.0   ", "+1", TODO
            ],
            scalar!(1.0),
        );

        parse_as_expression(
            &[
                "0.2", "  0.2  ", // ".2", "+.2", "+0.2 ", TODO
            ],
            scalar!(0.2),
        );

        parse_as_expression(
            &[
                "3.5", "  3.5  ", "3.50",
                // "+3.5", // TODO
            ],
            scalar!(3.5),
        );

        parse_as_expression(
            &[
                "0.05", // , ".05" TODO
            ],
            scalar!(0.05),
        );

        parse_as_expression(&["123.456"], scalar!(123.456));

        should_fail(&["123..", "0..", ".0.", ".", ". 2", "..2", ".."]);
    }

    #[test]
    fn large_numbers() {
        parse_as_expression(
            &[
                "1234567890000000",
                "1234567890000000.0",
                // "+1234567890000000.0", TODO
            ],
            scalar!(1234567890000000.0),
        );
    }

    #[test]
    fn negation() {
        parse_as_expression(&["-1", "  - 1   "], negate!(scalar!(1.0)));
        parse_as_expression(&["-123.45"], negate!(scalar!(123.45)));
        parse_as_expression(&["--1", " -  - 1   "], negate!(negate!(scalar!(1.0))));
        parse_as_expression(&["-x", " - x"], negate!(identifier!("x")));

        parse_as_expression(
            &["-1 + 2"],
            binop!(negate!(scalar!(1.0)), Add, scalar!(2.0)),
        );
    }

    #[test]
    fn identifiers() {
        parse_as_expression(&["foo", "  foo   "], identifier!("foo"));
        parse_as_expression(&["foo_bar"], identifier!("foo_bar"));
        parse_as_expression(&["MeineSchöneVariable"], identifier!("MeineSchöneVariable"));
        parse_as_expression(&["°"], identifier!("°"));
    }

    #[test]
    fn addition_and_subtraction() {
        parse_as_expression(
            &["1+2", "  1   +  2    "],
            binop!(scalar!(1.0), Add, scalar!(2.0)),
        );

        // Minus should be left-associative
        parse_as_expression(
            &["1-2-3", "1 - 2 - 3", "(1-2)-3"],
            binop!(binop!(scalar!(1.0), Sub, scalar!(2.0)), Sub, scalar!(3.0)),
        );

        should_fail(&["1+", "1-"]);
    }

    #[test]
    fn multiplication_and_division() {
        parse_as_expression(
            &["1*2", "  1   *  2    ", "1 · 2", "1 × 2"],
            binop!(scalar!(1.0), Mul, scalar!(2.0)),
        );

        parse_as_expression(
            &["1/2", "1 per 2", "1÷2"],
            binop!(scalar!(1.0), Div, scalar!(2.0)),
        );

        should_fail(&["1*@", "1*", "1 per", "÷", "×"]);
    }

    #[test]
    fn implicit_multiplication() {
        parse_as_expression(
            &["1 2", "  1     2    "],
            binop!(scalar!(1.0), Mul, scalar!(2.0)),
        );

        parse_as_expression(
            &["2 meter"],
            binop!(scalar!(2.0), Mul, identifier!("meter")),
        );
    }

    #[test]
    fn exponentiation() {
        parse_as_expression(
            &["2^3", "  2   ^  3    "],
            binop!(scalar!(2.0), Power, scalar!(3.0)),
        );

        parse_as_expression(
            &["2^3^4", "  2   ^  3   ^ 4 "],
            binop!(
                scalar!(2.0),
                Power,
                binop!(scalar!(3.0), Power, scalar!(4.0))
            ),
        );

        parse_as_expression(
            &["(2^3)^4"],
            binop!(
                binop!(scalar!(2.0), Power, scalar!(3.0)),
                Power,
                scalar!(4.0)
            ),
        );

        should_fail(&["1^", "1^^2"]);
    }

    #[test]
    fn unicode_exponentiation() {
        parse_as_expression(&["2³"], binop!(scalar!(2.0), Power, scalar!(3.0)));

        parse_as_expression(&["2⁻⁴"], binop!(scalar!(2.0), Power, scalar!(-4.0)));

        parse_as_expression(
            &["(2^3)²"],
            binop!(
                binop!(scalar!(2.0), Power, scalar!(3.0)),
                Power,
                scalar!(2.0)
            ),
        );

        parse_as_expression(
            &["2⁵^4"],
            binop!(
                binop!(scalar!(2.0), Power, scalar!(5.0)),
                Power,
                scalar!(4.0)
            ),
        );

        should_fail(&["1²³", "2⁻", "2⁻3"]);
    }

    #[test]
    fn conversion() {
        parse_as_expression(
            &["1->2", "1→2"],
            binop!(scalar!(1.0), ConvertTo, scalar!(2.0)),
        );

        // Conversion is left-associative
        parse_as_expression(
            &["1→2→3"],
            binop!(
                binop!(scalar!(1.0), ConvertTo, scalar!(2.0)),
                ConvertTo,
                scalar!(3.0)
            ),
        );

        should_fail(&["1 - > 2", "1 -> -> 2"]);
    }

    #[test]
    fn grouping() {
        parse_as_expression(
            &["1*(2+3)", "1 * ( 2 + 3 )"],
            binop!(scalar!(1.0), Mul, binop!(scalar!(2.0), Add, scalar!(3.0))),
        );

        should_fail(&["1 * (2 + 3", "2 + 3)"]);
    }

    #[test]
    fn variable_declaration() {
        parse_as(
            &["let foo = 1", "let foo=1"],
            Statement::DeclareVariable("foo".into(), scalar!(1.0), None),
        );

        parse_as(
            &["let x: Length = 1 * meter"],
            Statement::DeclareVariable(
                "x".into(),
                binop!(scalar!(1.0), Mul, identifier!("meter")),
                Some(DimensionExpression::Dimension("Length".into())),
            ),
        );

        should_fail_with(
            &["let (foo)=2", "let 2=3", "let = 2"],
            ParseErrorKind::ExpectedIdentifierAfterLet,
        );

        should_fail_with(
            &["let foo", "let foo 2"],
            ParseErrorKind::ExpectedEqualOrColonAfterLetIdentifier,
        );
    }

    #[test]
    fn dimension_declaration() {
        parse_as(
            &["dimension px"],
            Statement::DeclareDimension("px".into(), vec![]),
        );

        parse_as(
            &[
                "dimension Area = Length * Length",
                "dimension Area = Length × Length",
            ],
            Statement::DeclareDimension(
                "Area".into(),
                vec![DimensionExpression::Multiply(
                    Box::new(DimensionExpression::Dimension("Length".into())),
                    Box::new(DimensionExpression::Dimension("Length".into())),
                )],
            ),
        );

        parse_as(
            &["dimension Speed = Length / Time"],
            Statement::DeclareDimension(
                "Speed".into(),
                vec![DimensionExpression::Divide(
                    Box::new(DimensionExpression::Dimension("Length".into())),
                    Box::new(DimensionExpression::Dimension("Time".into())),
                )],
            ),
        );

        parse_as(
            &["dimension Area = Length^2"],
            Statement::DeclareDimension(
                "Area".into(),
                vec![DimensionExpression::Power(
                    Box::new(DimensionExpression::Dimension("Length".into())),
                    Rational::from_integer(2),
                )],
            ),
        );

        parse_as(
            &["dimension Energy = Mass * Length^2 / Time^2"],
            Statement::DeclareDimension(
                "Energy".into(),
                vec![DimensionExpression::Divide(
                    Box::new(DimensionExpression::Multiply(
                        Box::new(DimensionExpression::Dimension("Mass".into())),
                        Box::new(DimensionExpression::Power(
                            Box::new(DimensionExpression::Dimension("Length".into())),
                            Rational::from_integer(2),
                        )),
                    )),
                    Box::new(DimensionExpression::Power(
                        Box::new(DimensionExpression::Dimension("Time".into())),
                        Rational::from_integer(2),
                    )),
                )],
            ),
        );
    }

    #[test]
    fn function_declaration() {
        parse_as(
            &["fn foo() = 1"],
            Statement::DeclareFunction("foo".into(), vec![], vec![], Some(scalar!(1.0)), None),
        );

        parse_as(
            &["fn foo() -> Scalar = 1"],
            Statement::DeclareFunction(
                "foo".into(),
                vec![],
                vec![],
                Some(scalar!(1.0)),
                Some(DimensionExpression::Dimension("Scalar".into())),
            ),
        );

        parse_as(
            &["fn foo(x) = 1"],
            Statement::DeclareFunction(
                "foo".into(),
                vec![],
                vec![("x".into(), None)],
                Some(scalar!(1.0)),
                None,
            ),
        );

        parse_as(
            &["fn foo(x, y, z) = 1"],
            Statement::DeclareFunction(
                "foo".into(),
                vec![],
                vec![("x".into(), None), ("y".into(), None), ("z".into(), None)],
                Some(scalar!(1.0)),
                None,
            ),
        );

        parse_as(
            &["fn foo(x: Length, y: Time, z: Length^3 · Time^2) -> Scalar = 1"],
            Statement::DeclareFunction(
                "foo".into(),
                vec![],
                vec![
                    (
                        "x".into(),
                        Some(DimensionExpression::Dimension("Length".into())),
                    ),
                    (
                        "y".into(),
                        Some(DimensionExpression::Dimension("Time".into())),
                    ),
                    (
                        "z".into(),
                        Some(DimensionExpression::Multiply(
                            Box::new(DimensionExpression::Power(
                                Box::new(DimensionExpression::Dimension("Length".into())),
                                Rational::new(3, 1),
                            )),
                            Box::new(DimensionExpression::Power(
                                Box::new(DimensionExpression::Dimension("Time".into())),
                                Rational::new(2, 1),
                            )),
                        )),
                    ),
                ],
                Some(scalar!(1.0)),
                Some(DimensionExpression::Dimension("Scalar".into())),
            ),
        );

        parse_as(
            &["fn foo<X>(x: X) = 1"],
            Statement::DeclareFunction(
                "foo".into(),
                vec!["X".into()],
                vec![("x".into(), Some(DimensionExpression::Dimension("X".into())))],
                Some(scalar!(1.0)),
                None,
            ),
        );
    }

    #[test]
    fn postfix_apply() {
        parse_as_expression(
            &["1 + 1 // foo"],
            Expression::FunctionCall("foo".into(), vec![binop!(scalar!(1.0), Add, scalar!(1.0))]),
        );
    }

    #[test]
    fn procedure_call() {
        parse_as(
            &["print(2)"],
            Statement::ProcedureCall(ProcedureKind::Print, vec![scalar!(2.0)]),
        );

        parse_as(
            &["print(2, 3, 4)"],
            Statement::ProcedureCall(
                ProcedureKind::Print,
                vec![scalar!(2.0), scalar!(3.0), scalar!(4.0)],
            ),
        );

        should_fail_with(
            &["print", "print 2"],
            ParseErrorKind::ExpectedLeftParenAfterProcedureName,
        );

        should_fail_with(&["1+print(2)"], ParseErrorKind::InlineProcedureUsage);

        should_fail_with(
            &["fn print() = 1"],
            ParseErrorKind::ExpectedIdentifierAfterFn,
        );
    }
}