# The page table is our own hash table

Open addressing with linear probing, and twice as many slots as frames: one flat array with no allocation after `new`, never more than half full, so runs stay short. Half the slots are always empty, and a removal moves later entries of its run back.
