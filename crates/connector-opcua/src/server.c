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

/* Adds to `server` the variable `node`, which holds the Double `value`, under the
 * Objects folder. */
UA_StatusCode shim_server_add_double(UA_Server *server, shim_node node,
                                     UA_Double value) {
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    UA_Variant_setScalar(&attributes.value, &value, &UA_TYPES[UA_TYPES_DOUBLE]);
    attributes.dataType = UA_TYPES[UA_TYPES_DOUBLE].typeId;
    attributes.accessLevel = UA_ACCESSLEVELMASK_READ;
    return UA_Server_addVariableNode(
        server, shim_node_id(node),
        UA_NODEID_NUMERIC(0, UA_NS0ID_OBJECTSFOLDER),
        UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES),
        UA_QUALIFIEDNAME(node.namespace, "value"),
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

/* Gives the request handle that the last send of `request` gave it. */
UA_UInt32 shim_read_handle(const UA_ReadRequest *request) {
    return request->requestHeader.requestHandle;
}
