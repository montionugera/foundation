/* The declarations that `shim.c` and `server.c` share. */
#ifndef CONNECTOR_OPCUA_SHIM_H
#define CONNECTOR_OPCUA_SHIM_H

#include <open62541/types.h>
#include <stddef.h>
#include <stdint.h>

/* A node of a server, as `ffi::Node` gives it: the name of `length` bytes at `name`
 * in `namespace`, or the number `number` when `name` is NULL. */
typedef struct {
    UA_UInt16 namespace;
    UA_UInt32 number;
    const UA_Byte *name;
    size_t length;
} shim_node;

/* A scalar of a type from Boolean to Double, as `ffi::Scalar` gives it. The field
 * that holds it is that of its kind: the `UA_DataTypeKind` of its type, which for
 * these types is also the index of the type in `UA_TYPES`. */
typedef union {
    UA_Boolean boolean;
    UA_SByte sbyte;
    UA_Byte byte;
    UA_Int16 int16;
    UA_UInt16 uint16;
    UA_Int32 int32;
    UA_UInt32 uint32;
    UA_Int64 int64;
    UA_UInt64 uint64;
    UA_Float float_;
    UA_Double double_;
} shim_scalar;

/* Gives the id of `node`, which borrows its name. */
static inline UA_NodeId shim_node_id(shim_node node) {
    if(node.name == NULL)
        return UA_NODEID_NUMERIC(node.namespace, node.number);
    UA_NodeId id = {node.namespace, UA_NODEIDTYPE_STRING, {.string = {0}}};
    id.identifier.string.length = node.length;
    id.identifier.string.data = (UA_Byte *)(uintptr_t)node.name;
    return id;
}

#endif
