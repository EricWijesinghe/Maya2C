package maya2c

import (
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"strings"
	"testing"
)

// vectorFile is the same file crates/address/tests/shared_vectors.rs reads.
type vectorFile struct {
	Valid []struct {
		Name    string `json:"name"`
		Network string `json:"network"`
		IDHex   string `json:"id_hex"`
		Text    string `json:"text"`
	} `json:"valid"`
	Refused []struct {
		Name          string `json:"name"`
		Text          string `json:"text"`
		ExpectNetwork string `json:"expect_network"`
		Error         string `json:"error"`
	} `json:"refused"`
}

func loadVectors(t *testing.T) vectorFile {
	t.Helper()
	raw, err := os.ReadFile("../../spec/address-vectors.json")
	if err != nil {
		t.Fatal(err)
	}
	var v vectorFile
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	return v
}

func networkNamed(t *testing.T, name string) Network {
	t.Helper()
	switch name {
	case "mainnet":
		return NetworkMainnet
	case "testnet":
		return NetworkTestnet
	case "devnet":
		return NetworkDevnet
	}
	t.Fatalf("unknown network in vectors: %s", name)
	return ""
}

func TestSharedVectorsEncodeAndDecodeExactly(t *testing.T) {
	v := loadVectors(t)
	if len(v.Valid) < 13 {
		t.Fatalf("vector file lost entries: %d", len(v.Valid))
	}
	for _, c := range v.Valid {
		network := networkNamed(t, c.Network)
		raw, err := hex.DecodeString(c.IDHex)
		if err != nil || len(raw) != AddressBytes {
			t.Fatalf("%s: bad id_hex", c.Name)
		}
		var id [AddressBytes]byte
		copy(id[:], raw)
		// The all-uppercase vector is an input form; the canonical output is lowercase.
		if got, want := EncodeAddress(network, id), strings.ToLower(c.Text); got != want {
			t.Errorf("%s: encode = %s, want %s", c.Name, got, want)
		}
		got, err := DecodeAddress(network, c.Text)
		if err != nil || got != id {
			t.Errorf("%s: decode = %x, %v", c.Name, got, err)
		}
		gotNet, gotAny, err := DecodeAnyAddress(c.Text)
		if err != nil || gotAny != id || gotNet != network {
			t.Errorf("%s: decode_any = %s, %x, %v", c.Name, gotNet, gotAny, err)
		}
	}
}

func TestRefusedVectorsFailWithTheNamedError(t *testing.T) {
	want := map[string]error{
		"BadChecksum":    ErrBadChecksum,
		"MixedCase":      ErrMixedCase,
		"NoSeparator":    ErrNoSeparator,
		"Payload":        ErrAddressPayload,
		"UnknownNetwork": ErrUnknownNetwork,
		"Version":        ErrAddressVersion,
		"WrongNetwork":   ErrWrongNetwork,
	}
	v := loadVectors(t)
	if len(v.Refused) < 8 {
		t.Fatalf("vector file lost refusals: %d", len(v.Refused))
	}
	for _, c := range v.Refused {
		sentinel, ok := want[c.Error]
		if !ok {
			t.Fatalf("%s: no sentinel for %s", c.Name, c.Error)
		}
		var err error
		if c.ExpectNetwork != "" {
			_, err = DecodeAddress(networkNamed(t, c.ExpectNetwork), c.Text)
		} else {
			_, _, err = DecodeAnyAddress(c.Text)
		}
		if !errors.Is(err, sentinel) {
			t.Errorf("%s: got %v, want %v", c.Name, err, sentinel)
		}
	}
}

func TestHexAddressAcceptsHexAndBech32mAndRefusesTheRest(t *testing.T) {
	const hexForm = "06a9e5fc36d639816b77a9ab6619575df857f67503a8e33d05256a124e47d7cd"
	const text = "maya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxsgd736f"

	for _, in := range []string{hexForm, "0x" + strings.ToUpper(hexForm), text} {
		got, err := hexAddress(in)
		if err != nil || got != hexForm {
			t.Errorf("hexAddress(%q) = %q, %v", in, got, err)
		}
	}
	typo := strings.Replace(text, "gd736f", "gd736g", 1)
	for _, in := range []string{typo, "abcd", strings.Repeat("z", 64)} {
		if _, err := hexAddress(in); err == nil {
			t.Errorf("hexAddress(%q) accepted a bad address", in)
		}
	}
}
