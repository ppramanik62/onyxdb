# The pool checks the page count in one place, on every fetch

`pin`, which every fetch and every `PageStore` call goes through, checks the page number against a copy of the header's page count. `HeaderMut` updates the copy when it is dropped, so the check is one atomic load and takes no latch. A page past the end gives `OutOfRange` from every path, not a short read from some and a range error from others.
