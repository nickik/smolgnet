#include <core.p4>

struct ingress_metadata_t { bit<16> port; bool drop; }
struct egress_metadata_t { bit<16> port; bool drop; bool broadcast; bool transit; bool decided; }

#include "../../p4-common/gdp_wire.p4"

control ingress(inout headers_t hdr, inout ingress_metadata_t ingress, inout egress_metadata_t egress) {
    action allow() { }
    action drop() { egress.drop = true; egress.decided = true; }
    action forward(bit<16> port) { egress.port = port; egress.transit = false; egress.decided = true; }

    table global_nodes {
        key = { hdr.global_addr.destination_hi: exact; hdr.global_addr.destination_lo: exact; }
        actions = { allow; drop; forward; }
        default_action = allow; size = 4096;
    }

    table local_nodes {
        key = { hdr.local_addr.destination: exact; }
        actions = { drop; forward; }
        default_action = drop; size = 4096;
    }

    /*
     * Once a router is discovered, unknown global destinations from non-router
     * ports are sent to that router. Local-form packets never use this table.
     */
    table default_router {
        key = { ingress.port: exact; }
        actions = { drop; forward; }
        default_action = drop; size = 16;
    }

    apply {
        egress.transit = false;
        egress.decided = false;
        if (hdr.global_addr.isValid()) {
            global_nodes.apply();
            if (egress.decided == false) {
                default_router.apply();
            }
        } else if (hdr.local_addr.isValid()) {
            local_nodes.apply();
        } else {
            egress.drop = true;
            egress.decided = true;
        }
    }
}
control egress(inout headers_t hdr, inout ingress_metadata_t ingress, inout egress_metadata_t egress) { }
SoftNPU(parse(), ingress(), egress()) main;
