# Slot order is item order, so slot numbers are not stable

Inserting or removing an item in a slotted page moves the 4-byte slots after it, so the code above keeps items sorted without moving their bytes. A slot number does not always name the same item, so nothing outside a page can point at an item by (page, slot), as PostgreSQL indexes do.
