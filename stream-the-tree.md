# Stream the tree

What if we tried to find a way to work with sedimentrees which never needs to
load the entire tree - or even an entire commit - into memory. I've been 
exploring this idea a bit and I think it's lead to an approach which
dramatically simplifies sync.

The fundamental task we are interested in is synchronizing very large numbers
of sedimentree documents. To give an idea of scale lets say a normal server
will have hundreds of millions of documents synchronized with thousands of
users. Each user will be synchronizing ~100,000 documents with the server and
we expect normal installations to have up to 10,000 concurrent users.

We cannot really have an architecture which has all the documents in memory.
Furthermore, we should assume that at any given time we are operating at the
maximum amount of memory the machine has. This seems like a good place to use
TigerBeetle style and allocate a fixed amount of memory up front and then stay
in that budget.

To do this we're going to need to think about the fundamental operations of 
sync. Let's think about what an individual connection does when synchronizing.

## One Connection

Sync looks roughly like this:

First, the application somehow decides what set of sedimentrees to run sync
against. Then the initiating peer (initiator) does this:

* initiator: For each document in the set, send the cover (minimal(ish) sedimentree
  objects required to contain the whole tree) to the remote (receiver)
* receiver: load the cover for each document in the batch
* receiver: for every object the receiver has which it thinks the remote does
  not have it sends that object. It also sends a list of items it wants
* initiator: sends back each want that the server sent

