/* The declarations that `shim.c` and `server.c` share. */
#ifndef CONNECTOR_OPCUA_SHIM_H
#define CONNECTOR_OPCUA_SHIM_H

#include <open62541/types.h>
#include <stddef.h>

/* A node of a server, as `ffi::Node` gives it: the name of `length` bytes at `name`
 * in `namespace`, or the number `number` when `name` is NULL. */
typedef struct {
    UA_UInt16 namespace;
    UA_UInt32 number;
    const UA_Byte *name;
    size_t length;
} shim_node;

/* Gives the id of `node`, which borrows its name. */
UA_NodeId shim_node_id(shim_node node);

#endif
