# Archive Fixtures

These fixtures are tiny production-format archives used by the content extractor tests.

- `plain-7zz.7z` was created with `7zz 26.02` from one file named `fixture-needle.txt`.
- `encrypted-header-7zz.7z` was created with `7zz 26.02`, `-psecret`, and `-mhe=on` from the same file.

The fixtures intentionally exercise real 7z stream metadata and encoded-header encryption paths instead of the synthetic byte builders in the unit tests.
