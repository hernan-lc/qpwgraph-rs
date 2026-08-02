#define _GNU_SOURCE

#include <alsa/asoundlib.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define ALSA_MAX_NODES 256
#define ALSA_MAX_PORTS 4096

struct alsa_node {
    uint32_t id;
    char name[256];
};

struct alsa_port {
    uint32_t id;
    uint32_t node_id;
    uint32_t direction;
    char name[256];
};

struct alsa_link {
    uint32_t output_port;
    uint32_t input_port;
};

struct alsa_snapshot {
    uint32_t node_count;
    uint32_t port_count;
    uint32_t link_count;
    struct alsa_node nodes[ALSA_MAX_NODES];
    struct alsa_port ports[ALSA_MAX_PORTS];
    struct alsa_link links[ALSA_MAX_PORTS];
};

struct alsa_shim {
    snd_seq_t *seq;
    int client_id;
};

static uint32_t port_id(int client, int port)
{
    return ((uint32_t)(client & 0xffff) << 16) | (uint32_t)(port & 0xffff);
}

static void split_port_id(uint32_t id, int *client, int *port)
{
    *client = (int)((id >> 16) & 0xffff);
    *port = (int)(id & 0xffff);
}

struct alsa_shim *alsa_shim_new(void)
{
    struct alsa_shim *shim = calloc(1, sizeof(*shim));
    if (shim == NULL) {
        return NULL;
    }
    if (snd_seq_open(&shim->seq, "default", SND_SEQ_OPEN_DUPLEX, 0) < 0) {
        free(shim);
        return NULL;
    }
    snd_seq_set_client_name(shim->seq, "qpwgraph-rs");
    shim->client_id = snd_seq_client_id(shim->seq);
    if (shim->client_id < 0) {
        snd_seq_close(shim->seq);
        free(shim);
        return NULL;
    }
    return shim;
}

void alsa_shim_free(struct alsa_shim *shim)
{
    if (shim == NULL) {
        return;
    }
    snd_seq_close(shim->seq);
    free(shim);
}

int alsa_shim_snapshot(struct alsa_shim *shim, struct alsa_snapshot *snapshot)
{
    snd_seq_client_info_t *client_info;
    snd_seq_port_info_t *port_info;
    int client;
    if (shim == NULL || snapshot == NULL) {
        return -EINVAL;
    }
    memset(snapshot, 0, sizeof(*snapshot));
    snd_seq_client_info_alloca(&client_info);
    snd_seq_client_info_set_client(client_info, -1);
    while (snd_seq_query_next_client(shim->seq, client_info) >= 0) {
        client = snd_seq_client_info_get_client(client_info);
        if (client == shim->client_id || snapshot->node_count >= ALSA_MAX_NODES) {
            continue;
        }
        snapshot->nodes[snapshot->node_count].id = (uint32_t)client;
        snprintf(snapshot->nodes[snapshot->node_count].name,
            sizeof(snapshot->nodes[snapshot->node_count].name), "%s",
            snd_seq_client_info_get_name(client_info));
        ++snapshot->node_count;

        snd_seq_port_info_alloca(&port_info);
        snd_seq_port_info_set_client(port_info, client);
        snd_seq_port_info_set_port(port_info, -1);
        while (snd_seq_query_next_port(shim->seq, port_info) >= 0) {
            unsigned int capability;
            if (snapshot->port_count >= ALSA_MAX_PORTS) {
                break;
            }
            capability = snd_seq_port_info_get_capability(port_info);
            if ((capability & (SND_SEQ_PORT_CAP_READ | SND_SEQ_PORT_CAP_WRITE)) == 0) {
                continue;
            }
            snapshot->ports[snapshot->port_count].id = port_id(client, snd_seq_port_info_get_port(port_info));
            snapshot->ports[snapshot->port_count].node_id = (uint32_t)client;
            snapshot->ports[snapshot->port_count].direction =
                (capability & SND_SEQ_PORT_CAP_READ) != 0 ? 1 : 0;
            snprintf(snapshot->ports[snapshot->port_count].name,
                sizeof(snapshot->ports[snapshot->port_count].name), "%s",
                snd_seq_port_info_get_name(port_info));
            ++snapshot->port_count;
        }
    }
    return 0;
}

int alsa_shim_connect(struct alsa_shim *shim, uint32_t output_port, uint32_t input_port)
{
    int output_client, output_number, input_client, input_number;
    snd_seq_port_subscribe_t *subscription;
    snd_seq_addr_t sender;
    snd_seq_addr_t destination;
    if (shim == NULL) {
        return -EINVAL;
    }
    split_port_id(output_port, &output_client, &output_number);
    split_port_id(input_port, &input_client, &input_number);
    sender.client = output_client;
    sender.port = output_number;
    destination.client = input_client;
    destination.port = input_number;
    snd_seq_port_subscribe_alloca(&subscription);
    snd_seq_port_subscribe_set_sender(subscription, &sender);
    snd_seq_port_subscribe_set_dest(subscription, &destination);
    return snd_seq_subscribe_port(shim->seq, subscription);
}

int alsa_shim_disconnect(struct alsa_shim *shim, uint32_t output_port, uint32_t input_port)
{
    int output_client, output_number, input_client, input_number;
    snd_seq_port_subscribe_t *subscription;
    snd_seq_addr_t sender;
    snd_seq_addr_t destination;
    if (shim == NULL) {
        return -EINVAL;
    }
    split_port_id(output_port, &output_client, &output_number);
    split_port_id(input_port, &input_client, &input_number);
    sender.client = output_client;
    sender.port = output_number;
    destination.client = input_client;
    destination.port = input_number;
    snd_seq_port_subscribe_alloca(&subscription);
    snd_seq_port_subscribe_set_sender(subscription, &sender);
    snd_seq_port_subscribe_set_dest(subscription, &destination);
    return snd_seq_unsubscribe_port(shim->seq, subscription);
}
