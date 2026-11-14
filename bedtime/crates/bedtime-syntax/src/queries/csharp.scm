(namespace_declaration name: (_) @name) @outline.namespace
(file_scoped_namespace_declaration name: (_) @name) @outline.namespace
(class_declaration name: (_) @name) @outline.type
(struct_declaration name: (_) @name) @outline.type
(record_declaration name: (_) @name) @outline.type
(interface_declaration name: (_) @name) @outline.interface
(enum_declaration name: (_) @name) @outline.type
(delegate_declaration name: (_) @name) @outline.type
(method_declaration name: (_) @name) @outline.method
(constructor_declaration name: (_) @name) @outline.method
(destructor_declaration name: (_) @name) @outline.method
(property_declaration name: (_) @name) @outline.field
(event_declaration name: (_) @name) @outline.field
(field_declaration (variable_declaration (variable_declarator name: (_) @name) @outline.field))
(event_field_declaration (variable_declaration (variable_declarator name: (_) @name) @outline.field))
(enum_member_declaration name: (_) @name) @outline.variant
