package maya2c

// Bech32m (BIP-350) text form of a 32-byte account address, with a network
// prefix. A port of crates/address (ADR-081). The shared vectors in
// spec/address-vectors.json are the agreement: a change here that the Rust crate
// would not make fails address_test.go.

import (
	"encoding/hex"
	"errors"
	"fmt"
	"strings"
)

// Network is a human-readable prefix: the part written before the '1'.
type Network string

const (
	// NetworkMainnet is the mainnet prefix, "maya".
	NetworkMainnet Network = "maya"
	// NetworkTestnet is the public testnet prefix, "tmaya".
	NetworkTestnet Network = "tmaya"
	// NetworkDevnet is the local development prefix, "dmaya".
	NetworkDevnet Network = "dmaya"
)

// AddressBytes is the length of an account id.
const AddressBytes = 32

const (
	bech32Charset  = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"
	bech32mConst   = 0x2bc830a3
	checksumLen    = 6
	maxAddressLen  = 90
	payloadSymbols = 52 // 256 bits in 5-bit groups, rounded up
	dataSymbols    = 1 + payloadSymbols
	addressVersion = 0
)

var bech32Generator = [5]uint32{0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3}

// Sentinel errors, one per refusal in crates/address. Test code matches them with
// errors.Is.
var (
	ErrAddressLength    = errors.New("address is too long; the limit is 90 characters")
	ErrMixedCase        = errors.New("address mixes upper and lower case")
	ErrNoSeparator      = errors.New("address has no '1' separator")
	ErrEmptyHRP         = errors.New("address has no network prefix before the '1'")
	ErrInvalidCharacter = errors.New("address contains a character that is not allowed")
	ErrBadChecksum      = errors.New("address checksum does not match; a character is mistyped")
	ErrWrongNetwork     = errors.New("address is for a different network")
	ErrUnknownNetwork   = errors.New("address prefix names no known network")
	ErrAddressVersion   = errors.New("address format version is not known")
	ErrAddressPayload   = errors.New("address payload is malformed")
)

func polymod(values []byte) uint32 {
	chk := uint32(1)
	for _, v := range values {
		top := chk >> 25
		next := ((chk & 0x01ffffff) << 5) ^ uint32(v)
		for i, g := range bech32Generator {
			if (top>>uint(i))&1 == 1 {
				next ^= g
			}
		}
		chk = next
	}
	return chk
}

func hrpExpand(hrp string) []byte {
	out := make([]byte, 0, 2*len(hrp)+1)
	for i := 0; i < len(hrp); i++ {
		out = append(out, hrp[i]>>5)
	}
	out = append(out, 0)
	for i := 0; i < len(hrp); i++ {
		out = append(out, hrp[i]&31)
	}
	return out
}

func createChecksum(hrp string, data []byte) [checksumLen]byte {
	values := append(hrpExpand(hrp), data...)
	values = append(values, make([]byte, checksumLen)...)
	residue := polymod(values) ^ bech32mConst
	var sum [checksumLen]byte
	for i := range sum {
		sum[i] = byte((residue >> uint(5*(checksumLen-1-i))) & 31)
	}
	return sum
}

// asciiLower lowers ASCII letters only, byte by byte, as the Rust crate does.
func asciiLower(s string) string {
	b := []byte(s)
	for i, c := range b {
		if c >= 'A' && c <= 'Z' {
			b[i] = c + 32
		}
	}
	return string(b)
}

// splitBech32m checks the checksum and returns the prefix and the data symbols
// without the checksum.
func splitBech32m(text string) (string, []byte, error) {
	if len(text) > maxAddressLen {
		return "", nil, fmt.Errorf("%w: %d characters", ErrAddressLength, len(text))
	}
	hasLower, hasUpper := false, false
	for i := 0; i < len(text); i++ {
		hasLower = hasLower || (text[i] >= 'a' && text[i] <= 'z')
		hasUpper = hasUpper || (text[i] >= 'A' && text[i] <= 'Z')
	}
	if hasLower && hasUpper {
		return "", nil, ErrMixedCase
	}
	lower := asciiLower(text)
	sep := strings.LastIndexByte(lower, '1')
	if sep < 0 {
		return "", nil, ErrNoSeparator
	}
	hrp := lower[:sep]
	if hrp == "" {
		return "", nil, ErrEmptyHRP
	}
	for i := 0; i < len(hrp); i++ {
		if hrp[i] < 33 || hrp[i] > 126 {
			return "", nil, fmt.Errorf("%w: %q", ErrInvalidCharacter, hrp[i])
		}
	}
	var data []byte
	for _, c := range lower[sep+1:] {
		idx := strings.IndexRune(bech32Charset, c)
		if idx < 0 {
			return "", nil, fmt.Errorf("%w: %q", ErrInvalidCharacter, c)
		}
		data = append(data, byte(idx))
	}
	if len(data) < checksumLen {
		return "", nil, ErrAddressPayload
	}
	checked := append(hrpExpand(hrp), data...)
	if polymod(checked) != bech32mConst {
		return "", nil, ErrBadChecksum
	}
	return hrp, data[:len(data)-checksumLen], nil
}

// toSymbols regroups bytes into 5-bit symbols, zero-padding the last group.
func toSymbols(id []byte) []byte {
	out := make([]byte, 0, payloadSymbols)
	var acc uint32
	bits := uint(0)
	for _, b := range id {
		acc = acc<<8 | uint32(b)
		bits += 8
		for bits >= 5 {
			bits -= 5
			out = append(out, byte((acc>>bits)&31))
		}
		acc &= (1 << bits) - 1
	}
	if bits > 0 {
		out = append(out, byte((acc<<(5-bits))&31))
	}
	return out
}

// fromSymbols regroups 5-bit symbols into exactly 32 bytes. Non-zero padding is
// refused, so a non-canonical string never decodes to a valid id.
func fromSymbols(symbols []byte) ([AddressBytes]byte, error) {
	var out [AddressBytes]byte
	n := 0
	var acc uint32
	bits := uint(0)
	for _, s := range symbols {
		acc = acc<<5 | uint32(s)
		bits += 5
		for bits >= 8 {
			bits -= 8
			if n >= AddressBytes {
				return out, ErrAddressPayload
			}
			out[n] = byte((acc >> bits) & 0xff)
			n++
		}
		acc &= (1 << bits) - 1
	}
	if bits >= 5 || acc != 0 || n != AddressBytes {
		return out, ErrAddressPayload
	}
	return out, nil
}

// EncodeAddress renders an account id as its Bech32m text on network.
func EncodeAddress(network Network, id [AddressBytes]byte) string {
	data := append([]byte{addressVersion}, toSymbols(id[:])...)
	checksum := createChecksum(string(network), data)
	var b strings.Builder
	b.WriteString(string(network))
	b.WriteByte('1')
	for _, s := range append(data, checksum[:]...) {
		b.WriteByte(bech32Charset[s])
	}
	return b.String()
}

func parseAddress(text string) (string, [AddressBytes]byte, error) {
	var none [AddressBytes]byte
	hrp, data, err := splitBech32m(text)
	if err != nil {
		return "", none, err
	}
	if len(data) != dataSymbols {
		return "", none, ErrAddressPayload
	}
	if data[0] != addressVersion {
		return "", none, fmt.Errorf("%w: version %d", ErrAddressVersion, data[0])
	}
	id, err := fromSymbols(data[1:])
	return hrp, id, err
}

// DecodeAddress decodes text, which must be for expected.
func DecodeAddress(expected Network, text string) ([AddressBytes]byte, error) {
	hrp, id, err := parseAddress(text)
	if err != nil {
		return id, err
	}
	if hrp != string(expected) {
		return id, fmt.Errorf("%w: found %q, expected %q", ErrWrongNetwork, hrp, expected)
	}
	return id, nil
}

// DecodeAnyAddress decodes text for whichever known network its prefix names.
func DecodeAnyAddress(text string) (Network, [AddressBytes]byte, error) {
	hrp, id, err := parseAddress(text)
	if err != nil {
		return "", id, err
	}
	switch Network(hrp) {
	case NetworkMainnet, NetworkTestnet, NetworkDevnet:
		return Network(hrp), id, nil
	}
	return "", id, fmt.Errorf("%w: %q", ErrUnknownNetwork, hrp)
}

// hexAddress returns the 64-character lowercase hex form the gateway takes.
// It accepts that form (optionally 0x-prefixed) or any Bech32m address on a
// known network, and refuses anything else before a request is made.
func hexAddress(address string) (string, error) {
	bare := strings.TrimPrefix(address, "0x")
	if len(bare) == 2*AddressBytes {
		if _, err := hex.DecodeString(bare); err == nil {
			return strings.ToLower(bare), nil
		}
	}
	_, id, err := DecodeAnyAddress(address)
	if err != nil {
		return "", fmt.Errorf("invalid address: %w", err)
	}
	return hex.EncodeToString(id[:]), nil
}
