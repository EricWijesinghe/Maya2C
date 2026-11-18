// SdkError decoding: the uniffi error buffer mapped to typed C++ errors.
//
// These build the wire bytes by hand, so they pin the mapping without the
// Rust library. keys_test.cpp checks the same mapping through the real FFI.
#include <gtest/gtest.h>

#include <cstdint>
#include <vector>

#include "maya2c/detail/ffi.hpp"

namespace {

using maya2c::ErrorCode;
using maya2c::detail::decode_sdk_error;

void put_u32(std::vector<std::uint8_t>& out, std::uint32_t v) {
  out.push_back(static_cast<std::uint8_t>(v >> 24));
  out.push_back(static_cast<std::uint8_t>(v >> 16));
  out.push_back(static_cast<std::uint8_t>(v >> 8));
  out.push_back(static_cast<std::uint8_t>(v));
}

TEST(ErrorMapping, WrongLengthCarriesBothLengths) {
  std::vector<std::uint8_t> buf;
  put_u32(buf, 1);
  put_u32(buf, 32);
  put_u32(buf, 31);
  const auto error = decode_sdk_error(buf);
  EXPECT_EQ(error.code, ErrorCode::WrongLength);
  EXPECT_EQ(error.expected, 32u);
  EXPECT_EQ(error.actual, 31u);
}

TEST(ErrorMapping, MalformedCarriesWhat) {
  std::vector<std::uint8_t> buf;
  put_u32(buf, 2);
  put_u32(buf, 4);
  for (char c : {'s', 'e', 'e', 'd'}) buf.push_back(static_cast<std::uint8_t>(c));
  const auto error = decode_sdk_error(buf);
  EXPECT_EQ(error.code, ErrorCode::Malformed);
  EXPECT_NE(error.message.find("seed"), std::string::npos) << error.message;
}

TEST(ErrorMapping, VerificationFailedHasNoFields) {
  std::vector<std::uint8_t> buf;
  put_u32(buf, 3);
  EXPECT_EQ(decode_sdk_error(buf).code, ErrorCode::VerificationFailed);
}

TEST(ErrorMapping, AnUnknownVariantIsInternalNotAGuess) {
  std::vector<std::uint8_t> buf;
  put_u32(buf, 4);
  EXPECT_EQ(decode_sdk_error(buf).code, ErrorCode::Internal);
}

TEST(ErrorMapping, TruncatedBuffersAreInternal) {
  EXPECT_EQ(decode_sdk_error({}).code, ErrorCode::Internal);

  std::vector<std::uint8_t> wrong_length;
  put_u32(wrong_length, 1);
  put_u32(wrong_length, 32);  // `actual` missing
  EXPECT_EQ(decode_sdk_error(wrong_length).code, ErrorCode::Internal);

  std::vector<std::uint8_t> malformed;
  put_u32(malformed, 2);
  put_u32(malformed, 100);  // claims 100 bytes of text, carries none
  EXPECT_EQ(decode_sdk_error(malformed).code, ErrorCode::Internal);
}

}  // namespace
