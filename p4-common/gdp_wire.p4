/* Canonical GDP P4 wire definition. All P4 consumers include this file. */

header gdp_base_h {
    bit<2> version;
    bit<4> packet_type;
    bit<4> size_class;
    bit<1> local_form;
    bit<5> reserved;
    bit<8> crc8;
    bit<8> hop;
}

/* x4c represents each 64-bit GDP address as two 32-bit words. */
header gdp_global_h {
    bit<32> destination_hi;
    bit<32> destination_lo;
    bit<32> source_hi;
    bit<32> source_lo;
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
        if (hdr.base.local_form == 1w1) {
            transition parse_local;
        }
        transition parse_global;
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
