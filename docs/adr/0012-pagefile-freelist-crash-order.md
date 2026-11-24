# The freelist is a stack, and the page is written before the header

`free` puts a page at the front of the list, so freeing writes only that page and the header. `free` and `allocate` write the page before the header, so a crash between the two writes can leak a page but never gives one out twice. Pages allocated together can be far apart in the file, and `check` must find leaked pages.
