# The number of frames is fixed, and AllPinned is an error, not a wait

Memory for pages has a hard limit, and no fetch allocates. When every frame is pinned, a fetch returns `AllPinned` instead of waiting, because a thread that holds every pin itself would wait forever. The caller must handle the error.
