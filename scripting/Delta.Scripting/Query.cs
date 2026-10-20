using System;
using System.Collections.Generic;

namespace Delta;

/// <summary>
/// Describes a query: which components are read, written, required, or
/// excluded. Terms are exposed by component type through <see cref="Chunk"/>.
/// </summary>
public sealed unsafe class QueryBuilder
{
    readonly World world;
    readonly List<uint> read = [], write = [], optional = [], with = [], without = [], added = [], changed = [];

    internal QueryBuilder(World world) => this.world = world;

    public QueryBuilder Read<T>() where T : unmanaged, IComponent { read.Add(ComponentType<T>.Id); return this; }
    public QueryBuilder Write<T>() where T : unmanaged, IComponent { write.Add(ComponentType<T>.Id); return this; }
    public QueryBuilder Optional<T>() where T : unmanaged, IComponent { optional.Add(ComponentType<T>.Id); return this; }
    public QueryBuilder With<T>() { with.Add(ComponentType<T>.Id); return this; }
    public QueryBuilder Without<T>() { without.Add(ComponentType<T>.Id); return this; }
    public QueryBuilder Added<T>() { added.Add(ComponentType<T>.Id); return this; }
    public QueryBuilder Changed<T>() { changed.Add(ComponentType<T>.Id); return this; }

    /// <summary>Reads a Behaviour column (its handles); used by the behaviour dispatcher.</summary>
    internal QueryBuilder ReadRaw(uint component) { read.Add(component); return this; }

    public Query Build()
    {
        fixed (uint* r = read.ToArray(), w = write.ToArray(), o = optional.ToArray(),
               wi = with.ToArray(), wo = without.ToArray(), a = added.ToArray(), c = changed.ToArray())
        {
            var desc = new QueryDescFfi
            {
                Read = new IdList { Ptr = r, Len = (nuint)read.Count },
                Write = new IdList { Ptr = w, Len = (nuint)write.Count },
                Optional = new IdList { Ptr = o, Len = (nuint)optional.Count },
                With = new IdList { Ptr = wi, Len = (nuint)with.Count },
                Without = new IdList { Ptr = wo, Len = (nuint)without.Count },
                Added = new IdList { Ptr = a, Len = (nuint)added.Count },
                Changed = new IdList { Ptr = c, Len = (nuint)changed.Count },
            };
            var id = Host.Check(world.Ptr, Host.Api->QueryCreate(world.Ptr, &desc), "creating query");
            // Terms are numbered reads, then writes, then optionals.
            var terms = new List<uint>(read);
            terms.AddRange(write);
            terms.AddRange(optional);
            return new Query(world, id, [.. terms], read.Count, write.Count);
        }
    }
}

/// <summary>A compiled query. Iterate it chunk by chunk with <c>foreach</c>.</summary>
public sealed unsafe class Query
{
    internal readonly World World;
    internal readonly uint Id;
    internal readonly uint[] Terms;
    internal readonly int ReadCount;
    internal readonly int WriteCount;

    internal Query(World world, uint id, uint[] terms, int readCount, int writeCount)
    {
        World = world; Id = id; Terms = terms; ReadCount = readCount; WriteCount = writeCount;
    }

    public ChunkEnumerator GetEnumerator() => new(this);

    /// <summary>The term index of a component among reads (<paramref name="write"/> false) or writes.</summary>
    internal int TermOf(uint component, bool write)
    {
        var from = write ? ReadCount : 0;
        var to = write ? ReadCount + WriteCount : ReadCount;
        for (var i = from; i < to; i++)
            if (Terms[i] == component) return i;
        return -1;
    }

    internal int AnyTermOf(uint component)
    {
        var t = TermOf(component, false);
        if (t >= 0) return t;
        t = TermOf(component, true);
        if (t >= 0) return t;
        for (var i = ReadCount + WriteCount; i < Terms.Length; i++)
            if (Terms[i] == component) return i;
        return -1;
    }
}

public unsafe struct ChunkEnumerator
{
    readonly Query query;
    Cursor cursor;
    ChunkFfi ffi;

    internal ChunkEnumerator(Query query)
    {
        this.query = query;
        cursor = default;
        ffi = default;
    }

    public bool MoveNext()
    {
        fixed (Cursor* c = &cursor)
        fixed (ChunkFfi* f = &ffi)
            return Host.Api->QueryNext(query.World.Ptr, query.Id, c, f) != 0;
    }

    public readonly Chunk Current => new(query, ffi);
}

/// <summary>
/// A run of consecutive entities in one archetype, with direct spans over
/// the component columns. Valid until the system returns.
/// </summary>
public readonly unsafe ref struct Chunk
{
    readonly Query query;
    readonly ulong* entities;
    readonly ColumnPtrs columns;
    public int Count { get; }

    internal Chunk(Query query, ChunkFfi ffi)
    {
        this.query = query;
        entities = ffi.Entities;
        columns = ffi.Columns;
        Count = (int)ffi.Len;
    }

    public ReadOnlySpan<Entity> Entities => new(entities, Count);

    /// <summary>Mutable access to a component this query writes. The rows count as changed.</summary>
    public Span<T> Write<T>() where T : unmanaged, IComponent
    {
        var term = query.TermOf(ComponentType<T>.Id, write: true);
        if (term < 0) throw new InvalidOperationException($"{typeof(T).Name} is not a Write term of this query");
        return new Span<T>((void*)columns[term], Count);
    }

    /// <summary>Read-only access to a component this query reads (or writes).</summary>
    public ReadOnlySpan<T> Read<T>() where T : unmanaged, IComponent
    {
        var term = query.AnyTermOf(ComponentType<T>.Id);
        if (term < 0) throw new InvalidOperationException($"{typeof(T).Name} is not a term of this query");
        return new ReadOnlySpan<T>((void*)columns[term], Count);
    }

    /// <summary>For an Optional term: whether this chunk's archetype has the component.</summary>
    public bool Has<T>() where T : unmanaged, IComponent
    {
        var term = query.AnyTermOf(ComponentType<T>.Id);
        return term >= 0 && columns[term] != 0;
    }

    internal ReadOnlySpan<ulong> RawHandles(uint component)
    {
        var term = query.AnyTermOf(component);
        return new ReadOnlySpan<ulong>((void*)columns[term], Count);
    }
}
