# Volcano operators, and spilling to disk under a memory budget

Each operator is an iterator that pulls rows from the operators below it. Under a fixed memory budget, sorting becomes an external merge sort, and hash joins and aggregates spill partitions to disk, so a query bigger than memory still finishes.
