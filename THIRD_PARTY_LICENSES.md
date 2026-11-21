# Third-Party Licenses

This file lists third-party components bundled with or linked into gashuu, along with their
license terms. gashuu is grateful to the authors and contributors of these projects.

---

## UnRAR (RARLAB)

gashuu links the UnRAR sources statically via the [`unrar`](https://crates.io/crates/unrar)
Rust crate. UnRAR is used exclusively for **extracting** RAR/CBR archives; gashuu never creates
RAR archives and does not implement or incorporate any part of the RAR compression algorithm.

The UnRAR license is reproduced in full below:

```
 ******    *****   ******   UnRAR - free utility for RAR archives
 **   **  **   **  **   **  ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
 ******   *******  ******    License for use and distribution of
 **   **  **   **  **   **   ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
 **   **  **   **  **   **         FREE portable version
                                   ~~~~~~~~~~~~~~~~~~~~~

      The source code of UnRAR utility is freeware. This means:

   1. All copyrights to RAR and the utility UnRAR are exclusively
      owned by the author - Alexander Roshal.

   2. UnRAR source code may be used in any software to handle
      RAR archives without limitations free of charge, but cannot be
      used to develop RAR (WinRAR) compatible archiver and to
      re-create RAR compression algorithm, which is proprietary.
      Distribution of modified UnRAR source code in separate form
      or as a part of other software is permitted, provided that
      full text of this paragraph, starting from "UnRAR source code"
      words, is included in license, or in documentation if license
      is not available, and in source code comments of resulting package.

   3. The UnRAR utility may be freely distributed. It is allowed
      to distribute UnRAR inside of other software packages.

   4. THE RAR ARCHIVER AND THE UnRAR UTILITY ARE DISTRIBUTED "AS IS".
      NO WARRANTY OF ANY KIND IS EXPRESSED OR IMPLIED.  YOU USE AT
      YOUR OWN RISK. THE AUTHOR WILL NOT BE LIABLE FOR DATA LOSS,
      DAMAGES, LOSS OF PROFITS OR ANY OTHER KIND OF LOSS WHILE USING
      OR MISUSING THIS SOFTWARE.

   5. Installing and using the UnRAR utility signifies acceptance of
      these terms and conditions of the license.

   6. If you don't agree with terms of the license you must remove
      UnRAR files from your storage devices and cease to use the
      utility.

                                            Alexander L. Roshal
```

Source: https://www.rarlab.com/

---

## dav1d (VideoLAN)

gashuu links the dav1d AV1 decoder — statically in release builds, via the system library in
development builds — through the [`dav1d`](https://crates.io/crates/dav1d) Rust crate
(`dav1d-sys` bindings), enabled by the `image` crate's `avif-native` feature. dav1d is used
exclusively for **decoding** AVIF images; gashuu never encodes AVIF at runtime.

The dav1d license (BSD 2-Clause) is reproduced in full below:

```
Copyright © 2018-2024, VideoLAN and dav1d authors
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

Source: https://code.videolan.org/videolan/dav1d

---

## Noto Sans JP (SIL OFL 1.1)

gashuu ships this Noto Sans JP subset only in the web build, alongside its OFL license.

```
Copyright 2014-2021 Adobe (http://www.adobe.com/), with Reserved Font Name 'Source'

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
https://scripts.sil.org/OFL


-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

[Upstream OFL](https://github.com/google/fonts/blob/66a36c8c94b1a5d992ee4e7f392fccfe4945767c/ofl/notosansjp/OFL.txt)

---

## Other Major Dependencies

The following table lists other notable dependencies used by gashuu, together with their
commonly published license identifiers. Authoritative license texts are distributed alongside
each package on [crates.io](https://crates.io/).

| Crate | License |
|---|---|
| [`image`](https://crates.io/crates/image) | MIT / Apache-2.0 |
| [`dav1d`](https://crates.io/crates/dav1d) / [`dav1d-sys`](https://crates.io/crates/dav1d-sys) | MIT (bindings; the dav1d C library itself is BSD-2-Clause, see above) |
| [`mp4parse`](https://crates.io/crates/mp4parse) | MPL-2.0 |
| [`zip`](https://crates.io/crates/zip) | MIT |
| [`slint`](https://crates.io/crates/slint) | GPLv3 / Royalty-free / Commercial (tri-license — see [slint.dev/pricing](https://slint.dev/pricing)) |
| [`rfd`](https://crates.io/crates/rfd) | MIT |
| [`lru`](https://crates.io/crates/lru) | MIT |
| [`rayon`](https://crates.io/crates/rayon) | MIT / Apache-2.0 |
| [`serde`](https://crates.io/crates/serde) / [`serde_json`](https://crates.io/crates/serde_json) | MIT / Apache-2.0 |
| [`thiserror`](https://crates.io/crates/thiserror) | MIT / Apache-2.0 |
| [`tracing`](https://crates.io/crates/tracing) | MIT |
| [`color-eyre`](https://crates.io/crates/color-eyre) | MIT / Apache-2.0 |
| [`directories`](https://crates.io/crates/directories) | MIT / Apache-2.0 |
| [`walkdir`](https://crates.io/crates/walkdir) | MIT / Unlicense |

> **Note on Slint licensing:** Slint is available under the GNU General Public License v3
> (GPLv3) for free/open-source use, a royalty-free license for certain non-commercial or
> hobby projects, and a commercial license for proprietary products. See
> https://slint.dev/pricing for the complete terms.
