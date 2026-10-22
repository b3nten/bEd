(class_definition name: (_) @name) @outline.type
(function_definition name: (_) @name) @outline.function
((assignment left: (identifier) @name) @outline.constant (#match? @name "^[A-Z][A-Z0-9_]*$"))
