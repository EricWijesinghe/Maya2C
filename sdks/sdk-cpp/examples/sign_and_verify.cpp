// Generate a key, print its testnet address, sign and verify a message, and
// format an amount. Run: ./sign_and_verify
#include <cstdio>
#include <vector>

#include "maya2c/sdk.hpp"

int main() {
  auto key = maya2c::SigningKey::generate();
  if (!key) {
    std::fprintf(stderr, "generate: %s\n", key.error().message.c_str());
    return 1;
  }

  auto address = key.value().address(maya2c::Network::Testnet);
  if (!address) {
    std::fprintf(stderr, "address: %s\n", address.error().message.c_str());
    return 1;
  }
  std::printf("address:   %s\n", address.value().to_string().c_str());

  const std::vector<std::uint8_t> message{'h', 'e', 'l', 'l', 'o'};
  auto signature = key.value().sign(message);
  if (!signature) {
    std::fprintf(stderr, "sign: %s\n", signature.error().message.c_str());
    return 1;
  }
  std::printf("signature: %zu bytes\n", signature.value().size());

  auto public_key = key.value().public_key();
  if (!public_key) {
    std::fprintf(stderr, "public_key: %s\n", public_key.error().message.c_str());
    return 1;
  }
  auto verified = maya2c::verify(public_key.value(), message, signature.value());
  std::printf("verified:  %s\n", verified ? "yes" : verified.error().message.c_str());

  auto amount = maya2c::Amount::parse_coins("1.5");
  if (!amount) {
    std::fprintf(stderr, "amount: %s\n", amount.error().message.c_str());
    return 1;
  }
  std::printf("1.5 coins: %s base units\n", amount.value().to_base_units().c_str());
  return verified ? 0 : 1;
}
