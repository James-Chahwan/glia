import { mockDeep } from 'jest-mock-extended';
import { PrismaService } from '../prisma/prisma.service';
import { OrdersService } from './orders.service';

describe('OrdersService', () => {
  const mockPrisma = mockDeep<PrismaService>();
  const service = new OrdersService(mockPrisma);

  it('lists no orders when the store is empty', async () => {
    mockPrisma.order.findMany.mockResolvedValue([]);
    await expect(service.list()).resolves.toEqual([]);
  });
});
