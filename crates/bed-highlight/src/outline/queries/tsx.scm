(class_declaration name: (_) @name) @outline.type
(class name: (_) @name) @outline.type
(function_declaration name: (_) @name) @outline.function
(generator_function_declaration name: (_) @name) @outline.function
(method_definition name: (_) @name) @outline.method
(public_field_definition name: (_) @name) @outline.field
(variable_declarator name: (_) @name value: [(arrow_function) (function_expression) (generator_function)]) @outline.function
(pair key: (_) @name value: [(arrow_function) (function_expression) (generator_function)]) @outline.method
(assignment_expression left: (_) @name right: [(arrow_function) (function_expression)]) @outline.function
(lexical_declaration kind: "const" (variable_declarator name: (_) @name) @outline.constant)
(abstract_class_declaration name: (_) @name) @outline.type
(interface_declaration name: (_) @name) @outline.interface
(type_alias_declaration name: (_) @name) @outline.type
(enum_declaration name: (_) @name) @outline.type
(enum_body name: (_) @name @outline.variant)
(enum_assignment name: (_) @name) @outline.variant
(internal_module name: (_) @name) @outline.namespace
(module name: (_) @name) @outline.module
(function_signature name: (_) @name) @outline.function
(method_signature name: (_) @name) @outline.method
(abstract_method_signature name: (_) @name) @outline.method
(property_signature name: (_) @name) @outline.field
