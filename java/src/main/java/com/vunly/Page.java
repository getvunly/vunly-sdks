package com.vunly;

import java.util.List;

/**
 * One page of a list.
 *
 * @param nextCursor opaque: pass it back as {@link ListOptions#after(String)}. Null on the last page.
 */
public record Page<T>(List<T> data, String nextCursor) {
  public Page {
    data = List.copyOf(data);
  }

  /** True when another page follows. */
  public boolean hasMore() {
    return nextCursor != null && !nextCursor.isEmpty();
  }
}
