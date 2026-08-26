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

However, if we stream the cover of the tree out of storage in topological order
(specifically, in order of level descending, generation ascending, hash
ascending) then this becomes much easier. We're basically performing a merge of
two sorted streams. Something like this:

* initiator: For each document in the set, stream the cover to the remote
* receiver: when receiving the start of a cover, begin streaming the cover out
  of storage. Now traverse through both streams in parallel. For each object
  in the initiator stream if we have not encountered that in the local stream
  then emit a "want", for each object in the local stream which we have not
  encountered in the initiator stream then emit that object.

One nice property of this method:

* Individual network messages are bounded in size. Items in the cover are 
  purely a (level, generation, hash) tuple. This means that we can stream the
  cover in bounded memory

It does have an issue. We have a sort of head of line blocking problem. If
we're synchronizing many hundreds of documents we might want to sync the covers
first, then sync the contents. I'm not actually sure what this buys us though
so let's say we don't do that.

Of course, the rub is that we need to stream the document off disk in
topological order. This means we need to do some work on write. Let's assume
we've solved that though.

## Connections

Okay. I'm interested in how to handle many thousands of concurrent connections
in a small amount of memory. If we could get connection state down to 100 bytes
then we could handle 100,000 connections in a megabyte of RAM. This seems like 
a feasible "statically allocate it up front" amount.

What is the connection state that we actually need?

Well we need 8 bytes for a connection ID
(allows us up to 4 billion connections). But this can be an index into a static
array of connections so it's not state that's stored in the connection. 

### Receiving

We need to know if the connection is in a readable state. We also need to know
if there is data incoming for the connection. Here's how I think we handle it.
We maintain a queue of "ready" connections, once for which we think there is
data to read, and for which we have work to do. It's important that this is a
FIFO queue to maintain fairness.

Whenever we get a signal from the kernel (or wherever else) that there is data
to read for some connection we check if it also is ready to receive and if so
we enqueue the connection.

```rust
fn set_readable(&mut self, c: ConnId) {
    self.readable.insert(c);
    if self.recv_ready.contains(c) { self.enqueue(c); }
}
fn set_session_ready(&mut self, c: ConnId) {
    self.session_ready.insert(c);
    if self.recv_ready.contains(c) { self.enqueue(c); }
}
```

Actually receiving data is done by reading into a fixed pool of messages and
then passing an index to that message to the actual receive routine. This
means that when we are actually receiving a message we check three things:

* The TCP buffer has data to receive
* The connection is in a "receive ready" state
* There is a spot in the message pool

When do messages get discarded from the message pool? I don't know, I guess we
have to figure that out.






