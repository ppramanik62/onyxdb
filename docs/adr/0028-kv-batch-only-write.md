# A batch is the only way to write, and a crash keeps a prefix of whole batches

Every write goes through a `Batch` of puts and deletes, so "whole or not at all" has one meaning and each engine has one write path to make crash-safe. After a crash, an engine holds exactly some prefix of the applied batches, every synced batch included. A single put needs a batch around it.
