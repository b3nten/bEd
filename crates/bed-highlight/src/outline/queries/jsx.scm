(class_declaration name: (_) @name) @outline.type
(class name: (_) @name) @outline.type
(function_declaration name: (_) @name) @outline.function
(generator_function_declaration name: (_) @name) @outline.function
(method_definition name: (_) @name) @outline.method
(field_definition property: (_) @name) @outline.field
(variable_declarator name: (_) @name value: [(arrow_function) (function_expression) (generator_function)]) @outline.function
(pair key: (_) @name value: [(arrow_function) (function_expression) (generator_function)]) @outline.method
(assignment_expression left: (_) @name right: [(arrow_function) (function_expression)]) @outline.function
(lexical_declaration kind: "const" (variable_declarator name: (_) @name) @outline.constant)
