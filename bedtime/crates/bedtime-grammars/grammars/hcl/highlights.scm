; HCL (HashiCorp Configuration Language) highlights query

; Comments
(comment) @comment

; Strings and templates
(string_lit) @string
(quoted_template) @string
(heredoc_template) @string
(template_literal) @string

; Numbers
(numeric_lit) @number

; Booleans and null
(bool_lit) @constant.builtin
(null_lit) @constant.builtin

; Block type (first identifier in a block)
(block
  (identifier) @keyword)

; Attributes
(attribute
  (identifier) @property)

; Function calls
(function_call
  (identifier) @function.call)

; Get attribute access (like var.name)
(get_attr
  (identifier) @property)

; Variable expressions
(variable_expr
  (identifier) @variable)

; For expression identifiers
(for_intro
  (identifier) @variable)

; Template interpolation markers
(template_interpolation_start) @punctuation.special
(template_interpolation_end) @punctuation.special

; Template directive markers
(template_directive_start) @punctuation.special
(template_directive_end) @punctuation.special

; Heredoc markers
(heredoc_identifier) @string.special
(heredoc_start) @string.special

; Ellipsis (spread operator)
(ellipsis) @operator

; Operators
[
  "+"
  "-"
  "*"
  "/"
  "%"
  "=="
  "!="
  "<"
  ">"
  "<="
  ">="
  "&&"
  "||"
  "!"
  "?"
  ":"
  "=>"
] @operator

; Keywords (in template directives)
[
  "for"
  "in"
  "if"
  "else"
  "endif"
  "endfor"
] @keyword

; Punctuation
[
  "{"
  "}"
  "["
  "]"
  "("
  ")"
] @punctuation.bracket

[
  "="
  ","
  "."
] @punctuation.delimiter

; Fallback for identifiers
(identifier) @variable
