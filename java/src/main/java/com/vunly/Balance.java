package com.vunly;

/** A customer's credits. {@code remaining = granted - consumed} and is never negative. */
public record Balance(long remaining, long consumed, long granted) {
  static Balance from(Fields f) {
    return new Balance(f.lng("credits"), f.lng("consumed"), f.lng("granted"));
  }
}
