# Archive Fixtures

These fixtures are tiny production-format archives used by the content extractor tests.

- `plain-7zz.7z` was created with `7zz 26.02` from one file named `fixture-needle.txt`.
- `encrypted-header-7zz.7z` was created with `7zz 26.02`, `-psecret`, and `-mhe=on` from the same file.
- `split-7zz.7z.001` and `split-7zz.7z.002` were created with `7zz 26.02`, `-t7z`, `-mx=0`, and `-v1k` from one file named `split-fixture.txt`.

The fixtures intentionally exercise real 7z stream metadata, encoded-header encryption, and split-volume detection paths instead of the synthetic byte builders in the unit tests.
