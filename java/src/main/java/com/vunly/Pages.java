package com.vunly;

import java.util.Iterator;
import java.util.NoSuchElementException;
import java.util.function.Function;

/** Turns a page fetcher into an {@link Iterable} that fetches lazily; each {@code iterator()} starts from the first page. */
final class Pages {
  private static final int MAX_EMPTY_PAGES_IN_A_ROW = 3;

  private Pages() {}

  static <T> Iterable<T> iterable(Function<String, Page<T>> fetch) {
    return () ->
        new Iterator<>() {
          private Iterator<T> current = java.util.Collections.emptyIterator();
          private String cursor;
          private boolean exhausted;
          /** Cursors already used: a server that keeps handing back one must not make the iteration endless. */
          private final java.util.Set<String> seen = new java.util.HashSet<>();

          /** Pages in a row that had no items yet promised more: a fresh cursor every time must not make it endless either. */
          private int emptyInARow;

          @Override
          public boolean hasNext() {
            while (!current.hasNext() && !exhausted) {
              Page<T> p = fetch.apply(cursor);
              current = p.data().iterator();
              emptyInARow = p.data().isEmpty() ? emptyInARow + 1 : 0;
              if (p.hasMore() && emptyInARow >= MAX_EMPTY_PAGES_IN_A_ROW) {
                throw VunlyException.invalidResponse(
                    200, null, "too many empty pages in a row, each promising another");
              }
              if (p.hasMore()) {
                if (p.nextCursor() == null || !seen.add(p.nextCursor())) {
                  // the same page again: fail closed instead of looping, as with any other answer that cannot be right
                  throw VunlyException.invalidResponse(
                      200, null, "the next-page cursor did not advance");
                }
                cursor = p.nextCursor();
              } else {
                exhausted = true;
              }
            }
            return current.hasNext();
          }

          @Override
          public T next() {
            if (!hasNext()) {
              throw new NoSuchElementException();
            }
            return current.next();
          }
        };
  }
}
