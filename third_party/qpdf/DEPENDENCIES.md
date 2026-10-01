# Pinned SDK license provenance

The official qpdf 12.3.2 MSVC SDK is pinned by the archive SHA-256 in
scripts/build_qpdf.py. Its static link inventory includes JPEG, OpenSSL and zlib.
The latest nondeprecated qpdf/external-libs release at SDK publication was
`release-2026-01-08`; its source inventory names JPEG 9f, OpenSSL 3.6.0 and
zlib 1.3.1. The SDK does not publish a separate embedded-library SBOM, so these
versions are upstream build provenance, not independent runtime detection.

The following unmodified license texts were extracted from the official
[external-libs source release](https://github.com/qpdf/external-libs/releases/tag/release-2026-01-08):

* LICENSE-JPEG-9f.txt: jpeg-9f/README, including its legal-notice section.
* LICENSE-OpenSSL-3.6.0.txt: openssl-3.6.0/LICENSE.txt (Apache-2.0).
* LICENSE-zlib-1.3.1.txt: zlib-1.3.1/LICENSE.

Source archive `qpdf-external-libs-src.zip` SHA-256:
`88b70fdfb1c2041197a03751bbb2410d1dcda9c5c2b0b0e772930b376b98553c`.
It is only used to capture checked-in notices; it is not a build/runtime
dependency. qpdf's LICENSE.txt/NOTICE.md are from the v12.3.2 source tag.
MSBuild deploys every file in this directory under qpdf-notices.
Microsoft redistributable runtime DLLs retain their own applicable terms.
