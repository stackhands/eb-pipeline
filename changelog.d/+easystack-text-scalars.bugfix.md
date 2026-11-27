Keep a plain easystack scalar as text when EasyBuild keeps it as text.
A spelling such as `1e0` was read as the number one and became a bare option.
Spellings such as `1.5e1`, `+.5` and `0o20` changed into other numbers.
A spelling inside a longer plain scalar, such as `see 1e0 later`, `foo,1e0` or `1e0:80`, stays as written.
A repeated key written as `1e0`, `0o20`, `+.5` or `1.5e1` on its own line keeps that spelling, and the error points at the repeated key.
A duplicate inside a flow map uses the column in the original text.
