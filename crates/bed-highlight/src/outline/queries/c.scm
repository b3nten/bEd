(function_definition declarator: (_) @name) @outline.function
(struct_specifier name: (_) @name body: (_)) @outline.type
(union_specifier name: (_) @name body: (_)) @outline.type
(enum_specifier name: (_) @name body: (_)) @outline.type
(enumerator name: (_) @name) @outline.variant
(field_declaration declarator: (_) @name) @outline.field
(type_definition declarator: (_) @name) @outline.type
(preproc_def name: (_) @name) @outline.macro
(preproc_function_def name: (_) @name) @outline.macro
(declaration declarator: (function_declarator) @name) @outline.function
((declaration (type_qualifier) @qualifier declarator: (init_declarator declarator: (_) @name)) @outline.constant (#eq? @qualifier "const"))
