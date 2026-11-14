(module_declaration name: (_) @name) @outline.module
(class_declaration name: (_) @name) @outline.type
(record_declaration name: (_) @name) @outline.type
(interface_declaration name: (_) @name) @outline.interface
(annotation_type_declaration name: (_) @name) @outline.interface
(enum_declaration name: (_) @name) @outline.type
(enum_constant name: (_) @name) @outline.variant
(method_declaration name: (_) @name) @outline.method
(constructor_declaration name: (_) @name) @outline.method
(compact_constructor_declaration name: (_) @name) @outline.method
(annotation_type_element_declaration name: (_) @name) @outline.field
(field_declaration declarator: (variable_declarator name: (_) @name) @outline.field)
(constant_declaration declarator: (variable_declarator name: (_) @name) @outline.constant)
