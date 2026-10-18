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

/* A scalar of a type from Boolean to Double, as `ffi::Scalar` gives it, or the type of
 * another value. The field that holds a scalar is that of its kind: the
 * `UA_DataTypeKind` of its type, which for these types is also the index of the type in
 * `UA_TYPES`. */
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
    const UA_DataType *type;
} shim_scalar;

/* The bit that `shim_value_scalar` adds to the kind of an array. */
#define SHIM_ARRAY 0x8000

/* Gives the id of `node`, which borrows its name. */
static inline UA_NodeId shim_node_id(shim_node node) {
    if(node.name == NULL)
        return UA_NODEID_NUMERIC(node.namespace, node.number);
    UA_NodeId id = {node.namespace, UA_NODEIDTYPE_STRING, {.string = {0}}};
    id.identifier.string.length = node.length;
    id.identifier.string.data = (UA_Byte *)(uintptr_t)node.name;
    return id;
}

/* The certificate and private key (DER or PEM) of a peer, and the `trusted_size`
 * certificates of its trust list. */
typedef struct {
    UA_ByteString certificate;
    UA_ByteString key;
    const UA_ByteString *trusted;
    size_t trusted_size;
} shim_security;

struct UA_SecurityPolicy;
struct UA_CertificateGroup;
struct UA_EventLoop;
struct UA_Logger;

/* Adds the three policies of the copy on the certificate and key of `security` after
 * the `*size` policies of `*policies`. On a failure, `*size` counts the policies that
 * it added. */
UA_StatusCode shim_add_policies(struct UA_SecurityPolicy **policies, size_t *size,
                                const shim_security *security,
                                const struct UA_Logger *logger);

/* Makes `group`, which holds no group, a memory group of the default application
 * group whose trust list holds the `size` certificates of `trusted`, and which reads
 * the time of `el`. */
UA_StatusCode shim_trust(struct UA_CertificateGroup *group,
                         const UA_ByteString *trusted, size_t size,
                         const struct UA_Logger *logger, struct UA_EventLoop *el);

#endif
