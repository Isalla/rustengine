# Third-party notices

Code and assets that are not original to this repository, with the
license obligations they carry. Ordinary crate dependencies are covered
by `Cargo.lock` and shipped license texts upstream; this file lists
things that live inside the source tree.

## BC7 decoder (`crates/re-io/src/bc7.rs`)

Bit-layout and decode logic follow the reference implementation
[bcdec](https://github.com/iOrange/bcdec) by Sergii Kudlai, ported to
Rust. The 2- and 3-subset partition tables are taken from bcdec, which
derives them from the Microsoft BC7 specification.

bcdec is dual-licensed MIT / Unlicense; the MIT license text is included
below (choosing the MIT alternative for this port).

> MIT License
>
> Copyright (c) 2022 Sergii Kudlai
>
> Permission is hereby granted, free of charge, to any person obtaining
> a copy of this software and associated documentation files (the
> "Software"), to deal in the Software without restriction, including
> without limitation the rights to use, copy, modify, merge, publish,
> distribute, sublicense, and/or sell copies of the Software, and to
> permit persons to whom the Software is furnished to do so, subject to
> the following conditions:
>
> The above copyright notice and this permission notice shall be
> included in all copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
> EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
> MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
> IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
> CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
> TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
> SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## Inter font (`assets/fonts/inter/`)

Inter by Rasmus Andersson, licensed under the SIL Open Font License
1.1. The full license text ships next to the font files
(`assets/fonts/inter/OFL.txt`).
