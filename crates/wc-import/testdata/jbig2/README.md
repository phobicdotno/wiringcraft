# JBIG2 test streams

These streams come from the test suite of
[hayro-jbig2](https://github.com/LaurenzV/hayro/tree/main/hayro-jbig2/tests/streams)
(MIT OR Apache-2.0), at commit `e7ada1354dff4041cdeed3d8ba56fe01b6186132`. They are JBIG2 files in
the sequential organization; the tests strip the file header to get the embedded organization
that PDF uses.

| File | What it holds |
|---|---|
| `symbol.jbig2` | A symbol dictionary and a text region: a text-like scan of the "honeycomb" picture |
| `generic_trailing_7fff_stripped.jbig2` | The same picture as one generic region |
| `memory_symbol_dictionary_large_symbol.jbig2` | A symbol dictionary that declares one large symbol |
| `memory_symbol_dictionary_many_symbols.jbig2` | A symbol dictionary that declares many symbols |
