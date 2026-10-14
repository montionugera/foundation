/* The test server, which only a build with the feature `sim` compiles. No other C
   file of `src/` names a `UA_Server`, so a production build cannot reach its
   `setDefaultConfig`. */

/* The headers of the copy have unused parameters. Any other warning in them fails the
   build. */
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
#include <open62541/client.h>
#include <open62541/server_config_default.h>
#pragma GCC diagnostic pop

#include <string.h>

#include "shim.h"

/* Gives a server on `el` with the minimal config, the one server URL
 * `opc.tcp://{host}:{port}`, room for `sessions` sessions and secure channels, and at
 * most `chunks` chunks in a message, or NULL on a failure. */
UA_Server *shim_server_new(UA_EventLoop *el, UA_UInt16 port, const char *host,
                           UA_UInt16 sessions, UA_UInt32 chunks) {
    UA_ServerConfig config;
    memset(&config, 0, sizeof(config));
    /* The loop of `shim_loop_new` owns its logger, which is not const. */
    config.logging = (UA_Logger *)(uintptr_t)el->logger;
    config.eventLoop = el;
    config.externalEventLoop = true;
    if(UA_ServerConfig_setMinimal(&config, port, NULL) != UA_STATUSCODE_GOOD) {
        UA_ServerConfig_clear(&config);
        return NULL;
    }
    config.maxSessions = sessions;
    config.maxSecureChannels = sessions;
    config.tcpMaxChunks = chunks;
    /* The minimal config has the one server URL `opc.tcp://:{port}`. */
    UA_String *url = &config.serverUrls[0];
    UA_String_clear(url);
    if(UA_String_format(url, "opc.tcp://%s:%u", host, (unsigned)port) !=
       UA_STATUSCODE_GOOD) {
        UA_ServerConfig_clear(&config);
        return NULL;
    }
    return UA_Server_newWithConfig(&config);
}

/* Gives discovery URL `index` of `server`, or NULL past the last. */
const UA_String *shim_server_discovery_url(UA_Server *server, size_t index) {
    const UA_ApplicationDescription *description =
        &UA_Server_getConfig(server)->applicationDescription;
    if(index >= description->discoveryUrlsSize)
        return NULL;
    return &description->discoveryUrls[index];
}

/* Adds to `server` the variable `node`, which holds `value` of the kind `kind`, under
 * the Objects folder. */
UA_StatusCode shim_server_add_scalar(UA_Server *server, shim_node node,
                                     const shim_scalar *value, UA_UInt16 kind) {
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    UA_Variant_setScalar(&attributes.value, (void *)(uintptr_t)value, &UA_TYPES[kind]);
    attributes.dataType = UA_TYPES[kind].typeId;
    attributes.accessLevel = UA_ACCESSLEVELMASK_READ;
    return UA_Server_addVariableNode(
        server, shim_node_id(node),
        UA_NODEID_NUMERIC(0, UA_NS0ID_OBJECTSFOLDER),
        UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES),
        UA_QUALIFIEDNAME(node.namespace, "value"),
        UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATAVARIABLETYPE), attributes, NULL, NULL);
}

/* Adds to `server` the variable `node`, which holds an array of two Doubles, under the
 * Objects folder. */
UA_StatusCode shim_server_add_doubles(UA_Server *server, shim_node node) {
    UA_Double doubles[2] = {1.5, 2.5};
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    UA_Variant_setArray(&attributes.value, doubles, 2, &UA_TYPES[UA_TYPES_DOUBLE]);
    attributes.dataType = UA_TYPES[UA_TYPES_DOUBLE].typeId;
    attributes.accessLevel = UA_ACCESSLEVELMASK_READ;
    return UA_Server_addVariableNode(
        server, shim_node_id(node),
        UA_NODEID_NUMERIC(0, UA_NS0ID_OBJECTSFOLDER),
        UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES),
        UA_QUALIFIEDNAME(node.namespace, "doubles"),
        UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATAVARIABLETYPE), attributes, NULL, NULL);
}

/* Adds to `server` the variable `node`, which holds a ByteString of `size` zero
 * bytes, under the Objects folder. */
UA_StatusCode shim_server_add_bytes(UA_Server *server, shim_node node, size_t size) {
    UA_ByteString bytes;
    UA_StatusCode status = UA_ByteString_allocBuffer(&bytes, size);
    if(status != UA_STATUSCODE_GOOD)
        return status;
    memset(bytes.data, 0, size);
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    UA_Variant_setScalar(&attributes.value, &bytes, &UA_TYPES[UA_TYPES_BYTESTRING]);
    attributes.dataType = UA_TYPES[UA_TYPES_BYTESTRING].typeId;
    attributes.accessLevel = UA_ACCESSLEVELMASK_READ;
    status = UA_Server_addVariableNode(
        server, shim_node_id(node),
        UA_NODEID_NUMERIC(0, UA_NS0ID_OBJECTSFOLDER),
        UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES),
        UA_QUALIFIEDNAME(node.namespace, "bytes"),
        UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATAVARIABLETYPE), attributes, NULL, NULL);
    UA_ByteString_clear(&bytes);
    return status;
}

/* Sets the buffer size, the MaxMessageSize, and the MaxChunkCount of each later channel
 * of `server`, each 0 for the default of open62541. */
void shim_server_limits(UA_Server *server, UA_UInt32 buffer, UA_UInt32 message,
                        UA_UInt32 chunks) {
    UA_ServerConfig *config = UA_Server_getConfig(server);
    config->tcpBufSize = buffer;
    config->tcpMaxMsgSize = message;
    config->tcpMaxChunks = chunks;
}

/* Sets the receive buffer size that `client` gives in its next HEL. */
void shim_client_receive(UA_Client *client, UA_UInt32 size) {
    UA_Client_getConfig(client)->localConnectionConfig.recvBufferSize = size;
}

/* The node context of a variable of `shim_server_add_source`, as the first field of
 * the context that Rust holds. */
typedef struct shim_source shim_source;
struct shim_source {
    /* Gives the value, its kind, and the status of a read of the variable. */
    void (*read)(shim_source *source, shim_scalar *value, UA_UInt16 *kind,
                 UA_StatusCode *status);
};

static UA_StatusCode shim_source_read(UA_Server *server, const UA_NodeId *sessionId,
                                      void *sessionContext, const UA_NodeId *nodeId,
                                      void *nodeContext,
                                      UA_Boolean includeSourceTimeStamp,
                                      const UA_NumericRange *range,
                                      UA_DataValue *value) {
    (void)server;
    (void)sessionId;
    (void)sessionContext;
    (void)nodeId;
    (void)includeSourceTimeStamp;
    /* The client of the crate reads no range. */
    (void)range;
    value->hasStatus = true;
    shim_source *source = nodeContext;
    shim_scalar read;
    UA_UInt16 kind;
    source->read(source, &read, &kind, &value->status);
    UA_StatusCode status =
        UA_Variant_setScalarCopy(&value->value, &read, &UA_TYPES[kind]);
    value->hasValue = status == UA_STATUSCODE_GOOD;
    return status;
}

/* Adds to `server` the Double variable `node` under the Objects folder, whose reads
 * `source` answers. `source` lives until the server is deleted. */
UA_StatusCode shim_server_add_source(UA_Server *server, shim_node node,
                                     shim_source *source) {
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    attributes.dataType = UA_TYPES[UA_TYPES_DOUBLE].typeId;
    attributes.valueRank = UA_VALUERANK_SCALAR;
    attributes.accessLevel = UA_ACCESSLEVELMASK_READ;
    UA_CallbackValueSource callbacks = {.read = shim_source_read, .write = NULL};
    return UA_Server_addCallbackValueSourceVariableNode(
        server, shim_node_id(node),
        UA_NODEID_NUMERIC(0, UA_NS0ID_OBJECTSFOLDER),
        UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES),
        UA_QUALIFIEDNAME(node.namespace, "source"),
        UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATAVARIABLETYPE), attributes, callbacks,
        source, NULL);
}
