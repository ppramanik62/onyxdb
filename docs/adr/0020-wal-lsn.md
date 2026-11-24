# An LSN is the byte position just past a record, and segments are named by their first LSN

No record has LSN 0, so 0 can mean "never logged". Because segment files are named by the LSN where they start, `open` checks that they form one continuous run without reading them, and without knowing the segment size.
