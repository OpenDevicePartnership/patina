//! UEFI decompression.
//!
//! Implements UEFI decompression algorithms described in the UEFI Specification 2.11,
//! section 19. The entry point is [`decompress_into_with_algo`], which decompresses a
//! source buffer into a caller-provided destination buffer.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
// This module relies on extensive fixed-size table and tree indexing. Bounds are enforced
// by the algorithm's own invariants. Direct indexing may be replaced in the future but it
// is considered acceptable for now.
#![allow(clippy::indexing_slicing)]

use bitvec::{field::BitField, order::Msb0, slice::BitSlice, view::BitView};

/// Decompress Error Definitions
#[derive(Debug)]
pub enum DecompressError {
    /// The source buffer is too small or its declared compressed size is invalid.
    InvalidSrcSize,
    /// The destination buffer size does not match the declared original size.
    InvalidDstSize,
    /// The source data is malformed and cannot be decompressed.
    MalformedSrcData,
}

/// Supported Decompression Algorithms
#[derive(Debug)]
pub enum DecompressionAlgorithm {
    /// The UEFI decompression algorithm.
    UefiDecompress,
    /// The Tiano decompression algorithm.
    TianoDecompress,
}

/// Decompress the compressed data in `src` and store the output in `dst`, using the `algo` decompression algorithm.
pub fn decompress_into_with_algo(
    src: &[u8],
    dst: &mut [u8],
    algo: DecompressionAlgorithm,
) -> Result<(), DecompressError> {
    //sanity check the inputs
    if src.len() < 8 {
        Err(DecompressError::InvalidSrcSize)?;
    }

    let compressed_size = u32::from_le_bytes(src[0..4].try_into().unwrap()) as usize;
    if compressed_size > src.len() {
        Err(DecompressError::InvalidSrcSize)?;
    }

    let orig_size = u32::from_le_bytes(src[4..8].try_into().unwrap()) as usize;
    if orig_size == 0 {
        return Ok(());
    }
    if orig_size != dst.len() {
        Err(DecompressError::InvalidDstSize)?;
    }

    //Create a code iterator that iterates through the `src` bitstream and returns `CodeSymbol` elements.
    let mut dst_idx = 0;
    for result in CodeIterator::new(&src[8..], algo) {
        match result {
            Ok(symbol) => match symbol {
                CodeSymbol::OrigChar(char) => {
                    // symbol is an original character literal - copy it directly to the output buffer.
                    dst[dst_idx] = char;
                    dst_idx += 1;
                }
                CodeSymbol::StrPointer(offset, len) => {
                    // symbol is offset:len pair to be copied from a previously decompressed portion of the buffer.
                    let start = dst_idx
                        .checked_sub(offset)
                        .and_then(|x| x.checked_sub(1))
                        .ok_or(DecompressError::MalformedSrcData)?;

                    // note: this loop is used (instead of e.g. slice::copy_within or slice::copy_non_overlapping)
                    // because the offset:len window may overlap the current position. The "new" byte from the
                    // overlapping region needs to be copied instead of the original byte that existed at the start of
                    // the copy, which makes copy_within semantics inappropriate here.
                    for src in start..start + len {
                        dst[dst_idx] = dst[src];
                        dst_idx += 1;
                        if dst_idx == dst.len() {
                            break;
                        }
                    }
                }
            },
            //CodeIterator encountered an error trying to produce the next symbol - return it to caller.
            Err(err) => Err(err)?,
        }

        // Decompression is complete.
        if dst_idx == dst.len() {
            break;
        }
    }
    Ok(())
}

enum CodeSymbol {
    OrigChar(u8),
    StrPointer(usize, usize),
}

//Nomenclature: Char&Len set = 'C', Position set = 'P', Extra set = 'T'

//Size of Char&Len set
const NC: usize = 510;
const CBIT: usize = 9;
const CTABLE_BITSIZE: usize = 12;

//Size of Extra Set
const NT: usize = 19;
const TBIT: usize = 5;
const PTABLE_BITSIZE: usize = 8;

//Size of Position Set (actual size runtime defined based on selected algorithm)
const MAXNP: usize = 31;

const NPT: usize = [NT, MAXNP][(NT < MAXNP) as usize]; //Note: fancy const replacement for non-const usize::max(NT, MAXNP)

struct CodeIterator<'a> {
    src: &'a BitSlice<u8, Msb0>,
    src_index: usize,
    is_error: bool,
    remaining_block_size: usize,
    left: [u16; 2 * NC - 1],
    right: [u16; 2 * NC - 1],
    c_len: [u8; NC],
    pt_len: [u8; NPT],
    c_table: [u16; 1 << CTABLE_BITSIZE],
    pt_table: [u16; 1 << PTABLE_BITSIZE],
    p_bit: usize,
}

impl<'a> CodeIterator<'a> {
    // initialize a new CodeIterator instance for the given source and algorithm
    fn new(src: &'a [u8], algo: DecompressionAlgorithm) -> Self {
        Self {
            src: src.view_bits::<Msb0>(),
            src_index: 0,
            is_error: false,
            remaining_block_size: 0,
            left: [0u16; 2 * NC - 1],
            right: [0u16; 2 * NC - 1],
            c_len: [0u8; NC],
            pt_len: [0u8; NPT],
            c_table: [0u16; 4096],
            pt_table: [0u16; 256],
            p_bit: match algo {
                DecompressionAlgorithm::UefiDecompress => 4,
                DecompressionAlgorithm::TianoDecompress => 5,
            },
        }
    }

    // advances the source bitstream by `count` bits.
    fn pop_bits(&mut self, count: usize) -> Result<&BitSlice<u8, Msb0>, DecompressError> {
        if let Some(bitslice) = self.src.get(self.src_index..self.src_index + count) {
            self.src_index += count;
            Ok(bitslice)
        } else {
            Err(DecompressError::MalformedSrcData)
        }
    }

    // returns the next `count` bits of the source bitstream without advancing it.
    fn peek_bits(&self, count: usize) -> Result<&BitSlice<u8, Msb0>, DecompressError> {
        if let Some(bitslice) = self.src.get(self.src_index..self.src_index + count) {
            Ok(bitslice)
        } else {
            Err(DecompressError::MalformedSrcData)
        }
    }

    // Reads the code lengths for the Extra Set or Position Set Huffman codes for the current block.
    //
    // The code lengths are preceded by a `num_bits`-sized field that gives the length of the array.
    //
    // This is then followed by an encoded set of lengths which use a variable number of bits:
    // - If the code length is less than 7, it is encoded as a 3-bit binary number.
    // - If the code length is 7 or greater, it is encoded as a series of '1b' followed by a terminating '0b'.
    //   The code length is therefore equal to "count of 1s" + 4.
    //   Example: "4" is coded as '100b', "7" is coded as '1110b', and "12" is coded as `111111110b`
    //
    // If the 'extra' flag is set, then after the third length element in the bitstream, there is a 2-bit field
    // indicating the number of additional zero lengths that follow. For example, the following array of lengths
    // [2,9,0,0,5,7] would be encoded with the following bit stream (num_bits size field not shown).
    // 010 111110 10 101 1110
    //            ^
    //            this is the `extra` field added to generate the 2 "zero" lengths
    // If the extra flag is not set, the same array of lengths would be encoded with the following bitstream
    // 010 111110 000 000 101 1110
    //
    // The resulting code length array will be stored in self.pt_len.
    //
    // Once the code length array is generated, it is fed to the the Self::build_huffman_table() routine
    // to generate the resulting Huffman code table, which will be stored in self.pt_table.
    //
    // Refer to UEFI Specification 2.10, section 19.2.3.1.
    //
    fn read_pt_len(&mut self, num_symbols: usize, num_bits: usize, extra: bool) -> Result<(), DecompressError> {
        assert!(num_symbols <= NPT);

        // Read Set Length Array size
        let count = self.pop_bits(num_bits)?.load_be::<usize>();
        if count == 0 {
            // this represents the only Huffman code used.
            let char_c = self.pop_bits(num_bits)?.load_be::<u16>();
            self.pt_table.fill(char_c);
            self.pt_len[..num_symbols].fill(0);
            Ok(())
        } else {
            let mut idx = 0;
            while idx < count && idx < NPT {
                // if a code length is less than 7, it is encoded as 3-bit value. Otherwise it is encoded by a series of
                // 1s followed by a terminating zero. The number of 1s = code length - 4.
                let mut code_len = self.pop_bits(3)?.load_be::<u8>();
                if code_len == 7 {
                    loop {
                        let bit = self.pop_bits(1)?[0];
                        if bit {
                            //current bit is one.
                            code_len += 1;
                        } else {
                            break;
                        }
                    }
                }
                self.pt_len[idx] = code_len;
                idx += 1;

                // if 'extra' is set, then after the third length of the code length concatenation, a 2-bit value is
                // used to indicate the number of consecutive zero lengths immediately after the third length.
                if extra && idx == 3 {
                    let zero_count = self.pop_bits(2)?.load_be::<usize>();
                    self.pt_len[idx..idx + zero_count].fill(0);
                    idx += zero_count;
                }
            }
            if idx > num_symbols {
                Err(DecompressError::MalformedSrcData)?;
            }
            // zero the rest of the table.
            self.pt_len[idx..num_symbols].fill(0);

            //convert the resulting code length array (self.pt_len) into a Huffman coding table (self.pt_table)
            Self::build_huffman_table(
                num_symbols,
                &self.pt_len,
                PTABLE_BITSIZE,
                &mut self.pt_table,
                &mut self.left,
                &mut self.right,
            )
        }
    }

    // Read the code lengths for the Char&Length set Huffman code for the current block.
    //
    // The code lengths are preceded by a 9-bit field that gives the length of the array.
    //
    // This is then followed by an encoded set of lengths which use a variable number of bits. The set of lengths is
    // double-encoded:
    //
    //  1: If a code length is not zero, then it is encoded as "code length + 2";
    //     If a code length is zero, then the number of consecutive zero lengths starting from this code length is
    //     counted:
    //    - if the count is equal to or less than 2, then the code "0" is used for each zero length;
    //    - if the count is greater than 2 and less than 19, then the code "1" followed by a 4-bit value of "count - 3"
    //      is used for these consecutive zero lengths;
    //    - if the count is equal to 19, then it is treated as "1 + 18," and a code "0" and a code "1" followed by a
    //      4-bit value of "15" are used for these consecutive zero lengths;
    //    - if the count is greater than 19, then the code "2" followed by a 9-bit value of "count - 20" is used for
    //      these consecutive zero lengths.
    //  2: The resulting bitstring symbols are the "extra set", and are encoded using Huffman coding. The tables derived
    //     from execution of the read_pt_len() function on the extra set can be used to decode these symbols.
    //
    // To decode the table, the above process is reversed. First, the Huffman coded "extra set" symbols are decoded,
    // then the resulting symbols are converted into a code length by reversing the step 1 above.
    //
    // The resulting code length array will be stored in self.c_len.
    //
    // Once the code length array is generated, it is fed to the the Self::build_huffman_table() routine
    // to generate the resulting Huffman code table, which will be stored in self.c_table.
    //
    // Refer to UEFI Specification 2.10, section 19.2.3.1.
    //
    // NOTE: this routine requires that the current contents of self.pt_len, self.pt_table, self.left, and self.right
    // are initialized to match the "Extra Set" by executing read_pt_len() to decode the Extra Set Code Length Array.
    //
    fn read_c_len(&mut self) -> Result<(), DecompressError> {
        // Read Set Length Array Size
        let count = self.pop_bits(CBIT)?.load_be::<usize>();

        if count == 0 {
            // this represents the only Huffman code used
            let symbol = self.pop_bits(CBIT)?.load_be::<u16>();
            self.c_len.fill(0);
            self.c_table.fill(symbol);
            Ok(())
        } else {
            // iterate over all the symbols in the array.
            let mut idx = 0;
            while idx < count {
                // read the next symbol. First, read the first PTABLE_BITSIZE bits of the symbol.
                let mut symbol = self.pt_table[self.peek_bits(PTABLE_BITSIZE)?.load_be::<usize>()];
                // if the symbol is less than NT, then it can be used as-is
                if symbol as usize >= NT {
                    // symbol is larger than NT. Read bits from the stream and traverse the left/right tree until a leaf
                    // node (less than NT) is reached.
                    let mut mask_idx = PTABLE_BITSIZE;
                    loop {
                        let bit_buff = self.peek_bits(mask_idx + 1)?;
                        if bit_buff[mask_idx] {
                            symbol = self.right[symbol as usize];
                        } else {
                            symbol = self.left[symbol as usize];
                        }
                        mask_idx += 1;
                        if (symbol as usize) < NT {
                            break;
                        }
                    }
                }

                //now that we know the symbol, advance the bitstream by the symbol bitlength.
                self.pop_bits(self.pt_len[symbol as usize] as usize)?;

                if symbol <= 2 {
                    // if the symbol is 2 or less, it encodes 1 or more zero length symbols
                    if symbol == 0 {
                        // a single zero length
                        symbol = 1;
                    } else if symbol == 1 {
                        // '1' followed by a 4-bit value of count - 3 zero lengths follow.
                        symbol = self.pop_bits(4)?.load_be::<u16>() + 3;
                    } else if symbol == 2 {
                        // '2' followed by a 9-bit value of count - 20 zero lengths follow.
                        symbol = self.pop_bits(CBIT)?.load_be::<u16>() + 20;
                    }

                    //"symbol" now contains the consecutive number of zero-length symbols starting at the current idx.
                    //update the c_len table entries corresponding to these symbols and advance the index.
                    for _ in 0..symbol {
                        if idx >= self.c_len.len() {
                            Err(DecompressError::MalformedSrcData)?;
                        }
                        self.c_len[idx] = 0;
                        idx += 1;
                    }
                } else {
                    // otherwise, the symbol encodes 'code length +2'. store it in c_len and advance the index.
                    if idx >= self.c_len.len() {
                        Err(DecompressError::MalformedSrcData)?;
                    }
                    self.c_len[idx] = (symbol - 2) as u8;
                    idx += 1;
                }
            }
            // all valid symbols processed, zero the rest of c_len.
            self.c_len[idx..NC].fill(0);

            //convert the resulting code length array (self.c_len) into a Huffman coding table (self.c_table)
            Self::build_huffman_table(
                NC,
                &self.c_len,
                CTABLE_BITSIZE,
                &mut self.c_table,
                &mut self.left,
                &mut self.right,
            )
        }
    }

    // Decodes a "position" value from the current bitstream according to the Position Set encoding.
    //
    // A String Position is a value that indicates the distance between the current position and the target string. The
    // String Position value is defined as "Current Position - Starting Position of the target string - 1." The String
    // Position value ranges from 0 to 8190 (so 8192 is the "sliding window" size, and this range should be ensured by
    // the compressor). The lengths of the String Position values (in binary form) form a value set ranging from 0 to 13
    // (it is assumed that value 0 has length of 0). This value set is the Position Set for Huffman Coding. The full
    // representation of a String Position value is composed of two consecutive parts: one is the Huffman code for the
    // value length; the other is the actual String Position value of "length - 1" bits (excluding the highest bit since
    // the highest bit is always "1"). For example, String Position value 18 is represented as: Huffman code for "5"
    // followed by "0010." If the value length is 0 or 1, then no value is appended to the Huffman code.
    //
    // NOTE: this routine requires that the current contents of self.pt_len, self.pt_table, self.left, and self.right
    // are initialized to match the "Position Set" by executing read_pt_len() to decode the Position Set Code Length
    // Array.
    fn decode_position(&mut self) -> Result<usize, DecompressError> {
        //First, read the first PTABLE_BITSIZE bits of the position symbol.
        let bit_buffer = self.peek_bits(PTABLE_BITSIZE)?;
        let mut val = self.pt_table[bit_buffer.load_be::<usize>()] as usize;

        // if the symbol is less than NT, then it can be used as-is
        if val >= MAXNP {
            // symbol is larger than NT. Read bits from the stream and traverse the left/right tree until a leaf
            // node (less than NT) is reached.
            let mut mask_idx = PTABLE_BITSIZE;
            loop {
                let bit_buffer = self.peek_bits(mask_idx + 1)?;
                if bit_buffer[mask_idx] {
                    val = self.right[val] as usize;
                } else {
                    val = self.left[val] as usize;
                }

                mask_idx += 1;

                if val < MAXNP {
                    break;
                }
            }
        }
        self.pop_bits(self.pt_len[val] as usize)?;

        // if val is <= 1, then it directly encodes the position
        if val > 1 {
            // otherwise, (val - 1) encodes the bit length of an integer that encodes the position.
            val = (1 << (val - 1)) + self.pop_bits(val - 1)?.load_be::<usize>();
        }

        Ok(val)
    }

    // Constructs a Huffman decode table + tree.
    //
    // input parameters:
    // num_symbols: number of symbols in the Huffman symbol set
    // bit_lengths: a table describing the code length for each symbol (indexed by the symbol)
    // table_bits: the number of bits to be used for fixed symbol lookup. Symbols with an encoded bitlength longer than
    //             this parameter will require traversing the secondary tree to fully decode.
    //
    //  modifies:
    //  table: the fixed decode table (see description below)
    //  left: the "left" nodes of the secondary decoder tree.
    //  right: the right" nodes of the secondary decoder tree.
    //
    // This routine takes as input the bit_lengths table representing the canonical Huffman encoding over the output
    // symbols. It then generates 3 different table structures in the slices given as input:
    // - table: this table consists of two sets of entries.
    //    - fixed lookup entries - this consists of fixed entries for all symbols where the length of the encoded
    //      bitstring is less than or equal to the table_bits. For a given symbol, all entries that have that symbol as
    //      a prefix are set to the decoded value of the symbol. For example, assume that the bitstring `100b` is the
    //      encoded representation of the value 0xB - in that case, all of the entries of the table that start with
    //      `100xxxxxxxxxb` (i.e. indexes 0x800 to 0x9FF) would be set to 0xB.
    //    - tree lookup root entry - if the length of the encoded symbol is longer than the table bits, then the unique
    //      prefix of that entry points to the index of the root of a secondary decode tree encoded in the left & right
    //      array structures. "Leaf" elements of the tree occupy the first `num_symbol` entries in the left and right
    //      arrays, and correspond to literal final symbols. "Node" elements of the tree occupy the entries higher than
    //      `num_symbol` in the left and and right arrays and point to other nodes or leaves.
    //
    //      To decode the final symbol for an encoded bitstring that is longer than table_size bits, first locate the
    //      locate the entry within the table that corresponds to the root index in the left/right trees. Then, starting
    //      with the bit immediately following the first table_size bits of the encoded symbol, read bits from the
    //      encoded symbol. For each bit, if it is a 1, retrieve the next index from the `right` array, otherwise if it
    //      is a 0, retrieve the next index from the `left`. If the retrieved index is less than `num_symbol`, then it
    //      is the final decoded symbol. Otherwise, it is the index into the left or right tree for the next bit.
    //
    //      Note: if all possible symbols can be encoded within the fixed table width, then the secondary lookup is not
    //      needed.
    //
    // - left & right - the secondary decode tree as described above.
    //
    // Note: This implementation shares the "left & right" tables between the Char&Len symbol Set decode and the
    // Position Set decode; the portions of left & right used by each decode are disjoint. Care is taken to ensure that
    // constructing a table only modifies left & right indices associated with that table.
    fn build_huffman_table(
        num_symbols: usize,
        bit_lengths: &[u8],
        table_bits: usize,
        table: &mut [u16],
        left: &mut [u16],
        right: &mut [u16],
    ) -> Result<(), DecompressError> {
        assert!(table_bits <= 16);

        // calculate the number of symbols for each bit length.
        let mut count = [0u16; 17];
        for idx in 0..num_symbols {
            if bit_lengths[idx] > 16 {
                Err(DecompressError::MalformedSrcData)?;
            }
            count[bit_lengths[idx] as usize] += 1;
        }

        // Determine the start index for each bit length. This determines the start index within the fixed size decode
        // table for all symbols of a given bit length.
        let mut start = [0u16; 18];
        for idx in 1..=16 {
            let word_of_start = start[idx];
            let word_of_count = count[idx] << (16 - idx);
            start[idx + 1] = word_of_start.wrapping_add(word_of_count);
        }
        if start[17] != 0 {
            Err(DecompressError::MalformedSrcData)?;
        }

        // extended_bits is the number bits in the symbol exceeding the bit length for fixed entries in the table.
        let extended_bits = 16 - table_bits;

        // Determine weight of each length (the number of entries that a given symbol length will consume in the table).
        let mut weight = [0; 17];
        for idx in 1..=table_bits {
            start[idx] >>= extended_bits;
            weight[idx] = 1 << (table_bits - idx);
        }

        for (idx, w) in weight.iter_mut().enumerate().skip(table_bits + 1) {
            *w = 1 << (16 - idx);
        }

        // zero unused table entries.
        let idx = start[table_bits + 1] >> extended_bits;
        if idx != 0 {
            let idx_3 = 1 << table_bits;
            if idx < idx_3 {
                table[idx as usize..idx_3 as usize].fill(0);
            }
        }

        // Private helper structure used in the implementation below to simplify construction of the secondary tree.
        enum TablePointer {
            Table(usize),
            Left(usize),
            Right(usize),
        }
        impl TablePointer {
            fn set(&self, table: &mut [u16], left: &mut [u16], right: &mut [u16], val: u16) {
                match self {
                    TablePointer::Table(idx) => table[*idx] = val,
                    TablePointer::Left(idx) => left[*idx] = val,
                    TablePointer::Right(idx) => right[*idx] = val,
                }
            }

            fn get(&self, table: &mut [u16], left: &mut [u16], right: &mut [u16]) -> u16 {
                match self {
                    TablePointer::Table(idx) => table[*idx],
                    TablePointer::Left(idx) => left[*idx],
                    TablePointer::Right(idx) => right[*idx],
                }
            }
        }

        // tracks the next available node
        let mut next_avail_node = num_symbols;
        // mask used to check the bit for left vs. right construction
        let mask = 1 << (15 - table_bits);

        // iterate over all symbols in the alphabet to generate the table.
        for (char, sym_bit_len) in bit_lengths.iter().enumerate().take(num_symbols) {
            let sym_bit_len = *sym_bit_len as usize;

            // if the symbol length is zero, it is unused.
            if sym_bit_len == 0 {
                continue;
            }

            // max symbol length is fixed at 16 by spec, so encountering a larger symbol length is an error.
            if sym_bit_len > 16 {
                Err(DecompressError::MalformedSrcData)?;
            }

            // get the next code.
            let next_code = start[sym_bit_len].wrapping_add(weight[sym_bit_len]);

            if sym_bit_len <= table_bits {
                // the symbol is short enough that tree construction is not needed.

                // verify start and next sanity.
                if start[sym_bit_len] >= next_code || next_code > 1 << table_bits {
                    Err(DecompressError::MalformedSrcData)?;
                }

                // fill in all the elements in the table for which this symbol is a prefix.
                for idx in start[sym_bit_len]..next_code {
                    table[idx as usize] = char.try_into().expect("symbol count too large");
                }
            } else {
                // the symbol is long enough that tree construction is required.
                let mut symbol_bitstring = start[sym_bit_len];
                let mut pointer = TablePointer::Table((symbol_bitstring >> extended_bits) as usize);
                let mut idx = sym_bit_len - table_bits;

                // traverse the tree using the extended bits in the symbol bitstring to select nodes
                while idx != 0 {
                    if pointer.get(table, left, right) == 0 && next_avail_node < (2 * NC - 1) {
                        pointer.set(table, left, right, next_avail_node.try_into().expect("symbol count too large"));
                        right[next_avail_node] = 0;
                        left[next_avail_node] = 0;
                        next_avail_node += 1;
                    }

                    if pointer.get(table, left, right) < (2 * NC - 1) as u16 {
                        if symbol_bitstring & mask != 0 {
                            pointer = TablePointer::Right(pointer.get(table, left, right) as usize);
                        } else {
                            pointer = TablePointer::Left(pointer.get(table, left, right) as usize);
                        }
                    }

                    symbol_bitstring <<= 1;
                    idx -= 1;
                }
                // set the final node to the decoded symbol.
                pointer.set(table, left, right, char.try_into().expect("symbol count too large"));
            }

            //update the start index for this bit length
            start[sym_bit_len] = next_code;
        }
        Ok(())
    }
}

impl Iterator for CodeIterator<'_> {
    type Item = Result<CodeSymbol, DecompressError>;

    // Returns the next CodeSymbol from the bitstream.
    fn next(&mut self) -> Option<Self::Item> {
        if self.is_error {
            return None;
        }
        if self.remaining_block_size == 0 {
            //Starting a new block - re-initialize block state.

            //Read new block size.
            self.remaining_block_size = match self.pop_bits(16) {
                Ok(bits) => bits.load_be::<u16>() as usize,
                Err(err) => {
                    self.is_error = true;
                    return Some(Err(err));
                }
            };

            // Read in Extra Set Array and generate Huffman code mapping table for extra set used to decode Char&Len set.
            if let Err(err) = self.read_pt_len(NT, TBIT, true) {
                self.is_error = true;
                return Some(Err(err));
            }

            // Read in Char&Len Set Array and generate Huffman code mapping table for Char&Len set.
            if let Err(err) = self.read_c_len() {
                self.is_error = true;
                return Some(Err(err));
            }

            // Read in the Position Set Array and generate Huffman code mapping table for the Position set.
            if let Err(err) = self.read_pt_len(MAXNP, self.p_bit, false) {
                self.is_error = true;
                return Some(Err(err));
            }
        }
        self.remaining_block_size -= 1;

        // Decode the next Char&Len symbol. First, find the index in the c_table by peeking the next 12 bits.
        let bit_buff = match self.peek_bits(CTABLE_BITSIZE) {
            Ok(buff) => buff,
            Err(err) => {
                self.is_error = true;
                return Some(Err(err));
            }
        };
        let mut decode_idx = self.c_table[bit_buff.load_be::<usize>()] as usize;

        // If the index is larger than NC, then reconstruct the symbol by traversing the secondary decode tree.
        // see read_c_len() for details of how this is done.
        if decode_idx >= NC {
            let mut mask_idx = CTABLE_BITSIZE;
            loop {
                let bit_buff = match self.peek_bits(mask_idx + 1) {
                    Ok(buff) => buff,
                    Err(err) => {
                        self.is_error = true;
                        return Some(Err(err));
                    }
                };
                if bit_buff[mask_idx] {
                    decode_idx = self.right[decode_idx] as usize;
                } else {
                    decode_idx = self.left[decode_idx] as usize;
                }
                mask_idx += 1;
                if decode_idx < NC {
                    break;
                }
            }
        }
        //decode_idx the current symbol. Advance the bitstream by the bitlength of the current symbol.
        if let Err(err) = self.pop_bits(self.c_len[decode_idx] as usize) {
            self.is_error = true;
            return Some(Err(err));
        }

        //convert the symbol to the appropriate CodeSymbol
        if decode_idx < 256 {
            // symbols from 0-255 are byte literals.
            Some(Ok(CodeSymbol::OrigChar(decode_idx as u8)))
        } else {
            // symbols greater than 255 are string lengths.
            let len = decode_idx - (0x100 - 3);

            // string lengths are followed by an encoded string position; invoke decode_position() to decode it.
            let pos = match self.decode_position() {
                Ok(pos) => pos,
                Err(err) => {
                    self.is_error = true;
                    return Some(Err(err));
                }
            };

            Some(Ok(CodeSymbol::StrPointer(pos, len)))
        }
    }
}

#[cfg(test)]
mod test {
    use std::{fs::File, io::Read, iter::zip, println, time, vec, vec::Vec};

    use super::decompress_into_with_algo;

    macro_rules! test_collateral {
        ($fname:expr) => {
            concat!(env!("CARGO_MANIFEST_DIR"), "/test_resources/", $fname)
        };
    }

    #[test]
    fn uefi_decompress_should_produce_expected_buffer() {
        let mut compressed_file =
            File::open(test_collateral!("uefi_compressed.bin")).expect("failed to open test file");
        let mut compressed_buffer = Vec::new();

        compressed_file.read_to_end(&mut compressed_buffer).expect("failed to read test file");

        let mut uncompressed_file =
            File::open(test_collateral!("uefi_uncompressed.bin")).expect("failed to open test file");
        let mut uncompressed_buffer = Vec::new();
        uncompressed_file.read_to_end(&mut uncompressed_buffer).expect("failed to read test file");

        let mut test_buffer = vec![0u8; uncompressed_buffer.len()];

        decompress_into_with_algo(&compressed_buffer, &mut test_buffer, super::DecompressionAlgorithm::UefiDecompress)
            .unwrap();
        assert_eq!(test_buffer.len(), uncompressed_buffer.len());
        for (idx, (test, reference)) in zip(test_buffer, uncompressed_buffer).enumerate() {
            assert!(test == reference, "mismatch at idx: {idx:}, expected {reference:#x} != {test:#x} actual");
        }
    }

    #[test]
    fn tiano_decompress_should_produce_expected_buffer() {
        let mut compressed_file =
            File::open(test_collateral!("tiano_compressed.bin")).expect("failed to open test file");
        let mut compressed_buffer = Vec::new();

        compressed_file.read_to_end(&mut compressed_buffer).expect("failed to read test file");

        let mut uncompressed_file =
            File::open(test_collateral!("tiano_uncompressed.bin")).expect("failed to open test file");
        let mut uncompressed_buffer = Vec::new();
        uncompressed_file.read_to_end(&mut uncompressed_buffer).expect("failed to read test file");

        let mut test_buffer = vec![0u8; uncompressed_buffer.len()];

        decompress_into_with_algo(&compressed_buffer, &mut test_buffer, super::DecompressionAlgorithm::TianoDecompress)
            .unwrap();
        assert_eq!(test_buffer.len(), uncompressed_buffer.len());
        for (idx, (test, reference)) in zip(test_buffer, uncompressed_buffer).enumerate() {
            assert!(test == reference, "mismatch at idx: {idx:}, expected {reference:#x} != {test:#x} actual");
        }
    }

    #[test]
    fn decompress_with_original_size_of_zero_should_return_zero_sized_buffer() {
        // Setup a compressed buffer where the original size is zero but the compressed size is non-zero.
        // This is represented by a 16-byte buffer where the first byte is 0x08 (indicating compressed size is 8).
        let mut compressed_buffer = [0x0; 16];
        compressed_buffer[0] = 0x08;

        let mut uefi_uncompressed = Vec::new();
        assert!(
            decompress_into_with_algo(
                &compressed_buffer,
                &mut uefi_uncompressed,
                super::DecompressionAlgorithm::UefiDecompress
            )
            .is_ok()
        );
        assert_eq!(uefi_uncompressed.len(), 0);

        let mut tiano_uncompressed = Vec::new();
        assert!(
            decompress_into_with_algo(
                &compressed_buffer,
                &mut tiano_uncompressed,
                super::DecompressionAlgorithm::TianoDecompress
            )
            .is_ok()
        );
        assert_eq!(tiano_uncompressed.len(), 0);
    }

    #[test]
    fn fuzz_testing_should_fail_gracefully() {
        const FUZZ_COUNT: usize = 100;
        let mut compressed_file =
            File::open(test_collateral!("uefi_compressed.bin")).expect("failed to open test file");
        let mut compressed_buffer = Vec::new();

        compressed_file.read_to_end(&mut compressed_buffer).expect("failed to read test file");

        let mut uncompressed_file =
            File::open(test_collateral!("uefi_uncompressed.bin")).expect("failed to open test file");
        let mut uncompressed_buffer = Vec::new();
        uncompressed_file.read_to_end(&mut uncompressed_buffer).expect("failed to read test file");

        let uncompressed_len = uncompressed_buffer.len();

        for _ in 0..FUZZ_COUNT {
            let mut fuzz_buffer = compressed_buffer.clone();
            let fuzz_time = time::SystemTime::now().duration_since(time::UNIX_EPOCH).unwrap().as_micros() as usize;
            let fuzz_idx = fuzz_time % fuzz_buffer.len();
            println!("fuzz_idx: {:} before: {:#x}", fuzz_idx, fuzz_buffer[fuzz_idx]);
            fuzz_buffer[fuzz_idx] ^= 0xff;
            println!("fuzz_idx: {:} after: {:#x}", fuzz_idx, fuzz_buffer[fuzz_idx]);

            let mut test_buffer = vec![0u8; uncompressed_len];

            //note: not all corruption can be successfully detected. most of the time (but not all) this will return an Err.
            //the goal of the test is to ensure failure doesn't panic, not that bad data is always caught.
            let _ = decompress_into_with_algo(
                &fuzz_buffer,
                &mut test_buffer,
                super::DecompressionAlgorithm::UefiDecompress,
            );
        }
    }
}
