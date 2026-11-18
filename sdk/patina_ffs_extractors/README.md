# UEFI Section Extractor

A crate containing multiple UEFI section extractor implementations. It supports LZMA, Brotli, and CRC32 extraction.
These are all gated behind features as including them can greatly impact the binary size (primarily Brotli). In order
to use them in a platform, enable the given feature.
