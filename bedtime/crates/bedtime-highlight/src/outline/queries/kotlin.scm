(class_declaration (type_identifier) @name) @outline.type
(object_declaration (type_identifier) @name) @outline.type
(function_declaration (simple_identifier) @name) @outline.function
(type_alias (type_identifier) @name) @outline.type
(property_declaration (variable_declaration (simple_identifier) @name)) @outline.field
(class_parameter (binding_pattern_kind) (simple_identifier) @name) @outline.field
(enum_entry (simple_identifier) @name) @outline.variant
