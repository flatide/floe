# Desktop dependency inventory (D1 source/development build)

`desktop/Cargo.lock` is the resolved lock, separate from `rust/Cargo.lock`.
The shared registry packages retain the same version/checksum as the headless
workspace. `desktop/vendor/` reuses those unchanged packages through relative
symlinks into `rust/vendor/`; nine additional Cargo-vendored packages are below.
Keep the entire repository when building offline; do not copy `desktop/` alone.

| Package | Version | Upstream declared license |
|---|---|---|
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| block2 | 0.6.2 | MIT |
| dispatch2 | 0.3.1 | Zlib OR Apache-2.0 OR MIT |
| objc2 | 0.6.4 | MIT |
| objc2-encode | 4.1.0 | MIT |
| objc2-foundation | 0.3.2 | MIT |
| objc2-app-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-core-foundation | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-web-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT |

Original manifests, README files, source notices, Cargo package checksums and
`.cargo_vcs_info.json` are preserved without edits in each package. `bitflags`
includes its full license texts. The objc2 family packages refer to their parent
repository license document rather than bundling that document. See the
[upstream license at objc2 0.6.4's source commit](https://github.com/madsmtm/objc2/blob/8852b424193ca41602281b3d7540d7c8ed51e49a/LICENSE.md)
and [current upstream MIT text](https://github.com/madsmtm/objc2/blob/master/LICENSE-MIT.txt).
The MIT text below is a separately attributed upstream supplement, not a
modification to or a claim about the contents of the published crate archives.
Upstream also documents Apple SDK-derived binding considerations. System
AppKit/WebKit and Xcode SDK are not redistributed here; the host uses installed
macOS frameworks. Desktop distribution/notarization and final package notice
assembly remain D3 gates; this inventory does not declare legal review complete.

## objc2 upstream MIT supplement

Source: <https://raw.githubusercontent.com/madsmtm/objc2/master/LICENSE-MIT.txt>
retrieved 2026-09-18. Original text:

Copyright 2026 Mads Marquart

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:
The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.
THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
