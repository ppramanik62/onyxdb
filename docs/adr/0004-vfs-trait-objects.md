# vfs uses trait objects and positioned I/O

Code opens files through `&dyn Vfs` and holds `Box<dyn File>`, not a generic type parameter, because a generic would spread to every crate above. Files have only positioned, exact reads and writes (`read_at`, `write_at`), because a database reads whole pages at known offsets and a read then needs only `&self`. The cost is one indirect call per I/O and no streaming reads.
