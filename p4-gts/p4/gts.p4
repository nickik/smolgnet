#include <core.p4>

struct ingress_metadata_t { bit<16> port; bool drop; }
struct egress_metadata_t { bit<16> port; bool drop; bool broadcast; }

#include "../../p4-common/gts_wire.p4"

control ingress(inout headers_t hdr, inout ingress_metadata_t ingress, inout egress_metadata_t egress) {
    apply { egress.port = ingress.port; }
}
control egress(inout headers_t hdr, inout ingress_metadata_t ingress, inout egress_metadata_t egress) { }
SoftNPU(parse(), ingress(), egress()) main;
