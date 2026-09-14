#include <core.p4>

struct ingress_metadata_t {
    bit<16> port;
    bool drop;
}

struct egress_metadata_t {
    bit<16> port;
    bool drop;
    bool broadcast;
}

header gdp_base_h {
    bit<2> version;
    bit<4> packet_type;
    bit<4> size_class;
    bit<1> local_form;
    bit<5> reserved;
    bit<8> crc8;
    bit<8> hop;
}

header gdp_global_h {
    bit<64> destination;
    bit<64> source;
}

header gdp_local_h {
    bit<16> destination;
    bit<16> source;
}

struct headers_t {
    gdp_base_h base;
    gdp_global_h global_addr;
    gdp_local_h local_addr;
}

parser parse(
    packet_in pkt,
    out headers_t hdr,
    inout ingress_metadata_t ingress,
) {
    state start {
        pkt.extract(hdr.base);
        transition select(hdr.base.local_form) {
            1w1: parse_local;
            1w0: parse_global;
        }
    }

    state parse_global {
        pkt.extract(hdr.global_addr);
        transition accept;
    }

    state parse_local {
        pkt.extract(hdr.local_addr);
        transition accept;
    }
}

control ingress(
    inout headers_t hdr,
    inout ingress_metadata_t ingress,
    inout egress_metadata_t egress,
) {
    apply {
        // Phase 0/1 behavior is intentionally transparent.  Forwarding,
        // hop-limit mutation, CRC validation, and route tables are added only
        // after parser/deparser conformance with smolgnet is established.
        egress.port = ingress.port;
    }
}

control egress(
    inout headers_t hdr,
    inout ingress_metadata_t ingress,
    inout egress_metadata_t egress,
) {
}

SoftNPU(
    parse(),
    ingress(),
    egress()
) main;
