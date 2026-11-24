# Framed messages, and error codes that tell retry from fix

A frame is a type byte, a length and a body. Every error carries a code, so a client can tell an error that is worth retrying, such as a serialization failure, from one that needs a fix. Prepared statements are parsed and planned once, then run many times with bound `?` parameters.
