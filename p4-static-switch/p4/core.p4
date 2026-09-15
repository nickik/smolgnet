/* Minimal core declarations required by x4c's SoftNPU target. */
extern packet_in {
    void extract<T>(out T headerLvalue);
    void extract<T>(out T variableSizeHeader, in bit<32> varFieldSizeBits);
    T lookahead<T>();
    bit<32> length();
    void advance(bit<32> bits);
}

extern packet_out {
    void emit<T>(in T hdr);
}
